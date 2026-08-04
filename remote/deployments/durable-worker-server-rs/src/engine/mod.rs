use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    dag::{validate_identifier, validate_run_request, DagError},
    model::{
        CompleteStepRequest, DurableEvent, FailStepRequest, FailureRecord, IdempotencyRecord,
        JsonObject, LaneHolder, LaneRecord, LeaseCommand, LeaseRecord, MutationResponse,
        OutputReceipt, PollResponse, RunCounts, RunRecord, RunSnapshot, RunStatus, SignalResponse,
        StepAssignment, StepOutputRequest, StepRecord, StepStatus, SubmitRunRequest,
        SubmitRunResponse, WorkerHeartbeatRequest, WorkerRecord, WorkerRegistration, WorkerStatus,
    },
    store::{SharedEventSink, SharedStore, StoreError, StoredValue},
};

const MAX_CAS_RETRIES: usize = 32;
const DEFAULT_POLL_RETRY_MS: u64 = 250;
const MAX_OUTPUT_CHUNK_BYTES: usize = 1024 * 1024;

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default()
    }
}

#[derive(Default)]
pub struct ManualClock {
    now: AtomicU64,
}

impl ManualClock {
    pub fn new(now_ms: u64) -> Self {
        Self {
            now: AtomicU64::new(now_ms),
        }
    }

    pub fn set(&self, now_ms: u64) {
        self.now.store(now_ms, Ordering::SeqCst);
    }

    pub fn advance(&self, delta_ms: u64) {
        self.now.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub struct EngineMetrics {
    pub runs_submitted_total: AtomicU64,
    pub idempotent_replays_total: AtomicU64,
    pub worker_registrations_total: AtomicU64,
    pub leases_granted_total: AtomicU64,
    pub lease_conflicts_total: AtomicU64,
    pub lease_expirations_total: AtomicU64,
    pub step_timeouts_total: AtomicU64,
    pub step_completions_total: AtomicU64,
    pub step_failures_total: AtomicU64,
    pub retries_scheduled_total: AtomicU64,
    pub signals_total: AtomicU64,
    pub journal_failures_total: AtomicU64,
    pub scheduler_ticks_total: AtomicU64,
    pub scheduler_failures_total: AtomicU64,
}

impl EngineMetrics {
    pub fn render_prometheus(&self) -> String {
        let values = [
            (
                "dd_durable_runs_submitted_total",
                &self.runs_submitted_total,
            ),
            (
                "dd_durable_idempotent_replays_total",
                &self.idempotent_replays_total,
            ),
            (
                "dd_durable_worker_registrations_total",
                &self.worker_registrations_total,
            ),
            (
                "dd_durable_leases_granted_total",
                &self.leases_granted_total,
            ),
            (
                "dd_durable_lease_conflicts_total",
                &self.lease_conflicts_total,
            ),
            (
                "dd_durable_lease_expirations_total",
                &self.lease_expirations_total,
            ),
            ("dd_durable_step_timeouts_total", &self.step_timeouts_total),
            (
                "dd_durable_step_completions_total",
                &self.step_completions_total,
            ),
            ("dd_durable_step_failures_total", &self.step_failures_total),
            (
                "dd_durable_retries_scheduled_total",
                &self.retries_scheduled_total,
            ),
            ("dd_durable_signals_total", &self.signals_total),
            (
                "dd_durable_journal_failures_total",
                &self.journal_failures_total,
            ),
            (
                "dd_durable_scheduler_ticks_total",
                &self.scheduler_ticks_total,
            ),
            (
                "dd_durable_scheduler_failures_total",
                &self.scheduler_failures_total,
            ),
        ];
        let mut output = String::new();
        for (name, value) in values {
            output.push_str("# TYPE ");
            output.push_str(name);
            output.push_str(" counter\n");
            output.push_str(name);
            output.push(' ');
            output.push_str(&value.load(Ordering::Relaxed).to_string());
            output.push('\n');
        }
        output
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    InvalidGraph(#[from] DagError),
    #[error("{resource} not found: {id}")]
    NotFound { resource: &'static str, id: String },
    #[error("state conflict: {0}")]
    Conflict(String),
    #[error("idempotency key was already used with a different request")]
    IdempotencyMismatch,
    #[error("worker is offline or draining: {0}")]
    WorkerUnavailable(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

#[derive(Clone)]
pub struct Engine {
    store: SharedStore,
    events: SharedEventSink,
    clock: Arc<dyn Clock>,
    metrics: Arc<EngineMetrics>,
}

#[derive(Clone)]
struct Versioned<T> {
    revision: u64,
    value: T,
}

#[derive(Clone, Debug)]
struct ExpectedLease {
    worker_id: String,
    token: String,
    generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignalRecord {
    payload: JsonObject,
    created_at_ms: u64,
}

impl Engine {
    pub fn new(store: SharedStore, events: SharedEventSink, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            events,
            clock,
            metrics: Arc::new(EngineMetrics::default()),
        }
    }

    pub fn metrics(&self) -> Arc<EngineMetrics> {
        self.metrics.clone()
    }

    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    pub async fn ready(&self) -> bool {
        self.store.keys().await.is_ok()
    }

    pub async fn submit_run(
        &self,
        request: SubmitRunRequest,
    ) -> Result<SubmitRunResponse, EngineError> {
        validate_run_request(&request)?;
        let now = self.now_ms();
        let request_hash = stable_hash(&request)?;

        let (run_id, idempotent_replay) = if let Some(idempotency_key) =
            request.idempotency_key.as_deref()
        {
            let record_key = idempotency_key_key(idempotency_key);
            if let Some(existing) = self.load::<IdempotencyRecord>(&record_key).await? {
                if existing.value.request_hash != request_hash {
                    return Err(EngineError::IdempotencyMismatch);
                }
                self.metrics
                    .idempotent_replays_total
                    .fetch_add(1, Ordering::Relaxed);
                return self
                    .idempotent_submit_response(&existing.value.run_id)
                    .await;
            }

            let run_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, idempotency_key.as_bytes()).to_string();
            let record = IdempotencyRecord {
                run_id: run_id.clone(),
                request_hash: request_hash.clone(),
                created_at_ms: now,
            };
            match self.create_value(&record_key, &record).await {
                Ok(_) => (run_id, false),
                Err(EngineError::Store(StoreError::Conflict)) => {
                    let existing = self
                        .load::<IdempotencyRecord>(&record_key)
                        .await?
                        .ok_or_else(|| {
                            EngineError::Conflict(
                                "idempotency record changed during submission".to_string(),
                            )
                        })?;
                    if existing.value.request_hash != request_hash {
                        return Err(EngineError::IdempotencyMismatch);
                    }
                    self.metrics
                        .idempotent_replays_total
                        .fetch_add(1, Ordering::Relaxed);
                    return self
                        .idempotent_submit_response(&existing.value.run_id)
                        .await;
                }
                Err(error) => return Err(error),
            }
        } else {
            (Uuid::new_v4().to_string(), false)
        };

        if let Some(existing) = self.load::<RunRecord>(&run_key(&run_id)).await? {
            self.metrics
                .idempotent_replays_total
                .fetch_add(1, Ordering::Relaxed);
            return Ok(SubmitRunResponse {
                run_id,
                status: existing.value.status,
                idempotent_replay: true,
            });
        }

        let run_uuid = Uuid::parse_str(&run_id).map_err(|error| {
            EngineError::InvalidRequest(format!("generated run id is invalid: {error}"))
        })?;
        let mut step_ids = BTreeMap::new();
        let mut materialized = Vec::with_capacity(request.steps.len());
        for definition in &request.steps {
            let step_id = Uuid::new_v5(&run_uuid, definition.key.as_bytes()).to_string();
            step_ids.insert(definition.key.clone(), step_id.clone());
            let status = initial_step_status(definition, now);
            materialized.push(StepRecord {
                id: step_id,
                run_id: run_id.clone(),
                key: definition.key.clone(),
                task_type: definition.task_type.clone(),
                queue: definition.queue.clone(),
                input: definition.input.clone(),
                depends_on: definition.depends_on.clone(),
                priority: definition.priority,
                required_capabilities: definition.required_capabilities.clone(),
                retry: definition.retry.clone(),
                timeout_ms: definition.timeout_ms,
                lease_ms: definition.lease_ms,
                not_before_ms: definition.not_before_ms,
                wait_for_signal: definition.wait_for_signal.clone(),
                concurrency: definition.concurrency.clone(),
                affinity_key: definition.affinity_key.clone(),
                status,
                attempt: 0,
                lease_generation: 0,
                lease: None,
                last_lease: None,
                result: JsonObject::new(),
                failure: None,
                output_sequence: 0,
                last_output: None,
                created_at_ms: now,
                updated_at_ms: now,
                started_at_ms: None,
                completed_at_ms: None,
            });
        }

        for step in &materialized {
            match self.create_value(&step_key(&step.id), step).await {
                Ok(_) | Err(EngineError::Store(StoreError::Conflict)) => {}
                Err(error) => return Err(error),
            }
        }

        let run = RunRecord {
            id: run_id.clone(),
            name: request
                .name
                .unwrap_or_else(|| format!("run-{}", &run_id[..8])),
            status: RunStatus::Pending,
            metadata: request.metadata,
            step_ids,
            counts: counts_for_steps(&materialized),
            created_at_ms: now,
            updated_at_ms: now,
            completed_at_ms: None,
        };
        match self.create_value(&run_key(&run_id), &run).await {
            Ok(_) => {}
            Err(EngineError::Store(StoreError::Conflict)) => {
                let existing = self
                    .load::<RunRecord>(&run_key(&run_id))
                    .await?
                    .ok_or_else(|| {
                        EngineError::Conflict("run changed during submission".to_string())
                    })?;
                self.metrics
                    .idempotent_replays_total
                    .fetch_add(1, Ordering::Relaxed);
                return Ok(SubmitRunResponse {
                    run_id,
                    status: existing.value.status,
                    idempotent_replay: true,
                });
            }
            Err(error) => return Err(error),
        }

        self.metrics
            .runs_submitted_total
            .fetch_add(1, Ordering::Relaxed);
        self.publish_best_effort(self.event(
            "run.submitted",
            &run_id,
            None,
            None,
            "submitted",
            object(json!({
                "name": run.name,
                "stepCount": run.counts.total,
                "idempotent": idempotent_replay,
            })),
        ))
        .await;
        Ok(SubmitRunResponse {
            run_id,
            status: RunStatus::Pending,
            idempotent_replay,
        })
    }

    pub async fn get_run_snapshot(&self, run_id: &str) -> Result<RunSnapshot, EngineError> {
        let run = self
            .load::<RunRecord>(&run_key(run_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "run",
                id: run_id.to_string(),
            })?
            .value;
        let mut steps = Vec::with_capacity(run.step_ids.len());
        for step_id in run.step_ids.values() {
            steps.push(self.get_step(step_id).await?);
        }
        steps.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(RunSnapshot { run, steps })
    }

    pub async fn get_step(&self, step_id: &str) -> Result<StepRecord, EngineError> {
        self.load::<StepRecord>(&step_key(step_id))
            .await?
            .map(|record| record.value)
            .ok_or_else(|| EngineError::NotFound {
                resource: "step",
                id: step_id.to_string(),
            })
    }

    pub async fn register_worker(
        &self,
        mut registration: WorkerRegistration,
    ) -> Result<WorkerRecord, EngineError> {
        validate_identifier(&registration.worker_id, "workerId", 200)?;
        if registration.queues.is_empty() {
            registration.queues.insert("default".to_string());
        }
        for queue in &registration.queues {
            validate_identifier(queue, "queues[]", 128)?;
        }
        for capability in &registration.capabilities {
            validate_identifier(capability, "capabilities[]", 128)?;
        }
        if registration.slots == 0 || registration.slots > 10_000 {
            return Err(EngineError::InvalidRequest(
                "worker slots must be between 1 and 10000".to_string(),
            ));
        }
        if !(1_000..=24 * 60 * 60 * 1_000).contains(&registration.ttl_ms) {
            return Err(EngineError::InvalidRequest(
                "worker ttlMs must be between 1000 and 86400000".to_string(),
            ));
        }

        let key = worker_key(&registration.worker_id);
        let now = self.now_ms();
        for _ in 0..MAX_CAS_RETRIES {
            let current = self.load::<WorkerRecord>(&key).await?;
            let record = WorkerRecord {
                worker_id: registration.worker_id.clone(),
                queues: registration.queues.clone(),
                capabilities: registration.capabilities.clone(),
                labels: registration.labels.clone(),
                slots: registration.slots,
                ttl_ms: registration.ttl_ms,
                status: if registration.drain.unwrap_or(false) {
                    WorkerStatus::Draining
                } else {
                    WorkerStatus::Online
                },
                registered_at_ms: current
                    .as_ref()
                    .map(|record| record.value.registered_at_ms)
                    .unwrap_or(now),
                last_heartbeat_ms: now,
            };
            let result = if let Some(current) = current {
                self.update_value(&key, current.revision, &record).await
            } else {
                self.create_value(&key, &record).await
            };
            match result {
                Ok(_) => {
                    self.metrics
                        .worker_registrations_total
                        .fetch_add(1, Ordering::Relaxed);
                    return Ok(record);
                }
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "worker registration exceeded CAS retry budget".to_string(),
        ))
    }

    pub async fn heartbeat_worker(
        &self,
        worker_id: &str,
        request: WorkerHeartbeatRequest,
    ) -> Result<WorkerRecord, EngineError> {
        let key = worker_key(worker_id);
        let now = self.now_ms();
        for _ in 0..MAX_CAS_RETRIES {
            let mut current =
                self.load::<WorkerRecord>(&key)
                    .await?
                    .ok_or_else(|| EngineError::NotFound {
                        resource: "worker",
                        id: worker_id.to_string(),
                    })?;
            current.value.last_heartbeat_ms = now;
            current.value.status = match request.drain {
                Some(true) => WorkerStatus::Draining,
                Some(false) => WorkerStatus::Online,
                None if current.value.status == WorkerStatus::Offline => WorkerStatus::Online,
                None => current.value.status,
            };
            match self
                .update_value(&key, current.revision, &current.value)
                .await
            {
                Ok(_) => return Ok(current.value),
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "worker heartbeat exceeded CAS retry budget".to_string(),
        ))
    }

    pub async fn poll_once(&self, worker_id: &str) -> Result<PollResponse, EngineError> {
        let worker = self
            .load::<WorkerRecord>(&worker_key(worker_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "worker",
                id: worker_id.to_string(),
            })?
            .value;
        let now = self.now_ms();
        if worker.status != WorkerStatus::Online
            || now.saturating_sub(worker.last_heartbeat_ms) > worker.ttl_ms
        {
            return Err(EngineError::WorkerUnavailable(worker_id.to_string()));
        }

        let mut candidates = self.scan_steps().await?;
        candidates.retain(|step| {
            step.value.status == StepStatus::Queued && worker_matches(&worker, &step.value)
        });
        candidates.sort_by(|left, right| {
            right
                .value
                .priority
                .cmp(&left.value.priority)
                .then_with(|| left.value.created_at_ms.cmp(&right.value.created_at_ms))
                .then_with(|| left.value.id.cmp(&right.value.id))
        });

        for candidate in candidates {
            if let Some(assignment) = self.try_lease_step(&worker, &candidate.value.id).await? {
                return Ok(PollResponse {
                    assignment: Some(assignment),
                    retry_after_ms: 0,
                });
            }
        }

        Ok(PollResponse {
            assignment: None,
            retry_after_ms: DEFAULT_POLL_RETRY_MS,
        })
    }

    pub async fn start_step(
        &self,
        step_id: &str,
        command: LeaseCommand,
    ) -> Result<MutationResponse, EngineError> {
        let now = self.now_ms();
        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            if current.value.status == StepStatus::Running
                && current_lease_matches(&current.value, &command)
            {
                return Ok(step_mutation(&current.value));
            }
            validate_active_lease(&current.value, &command, now)?;
            if current.value.status != StepStatus::Leased
                && current.value.status != StepStatus::Running
            {
                return Err(EngineError::Conflict(format!(
                    "step {} is not leased",
                    current.value.id
                )));
            }
            current.value.status = StepStatus::Running;
            current.value.started_at_ms.get_or_insert(now);
            current.value.updated_at_ms = now;
            match self
                .update_value(&step_key(step_id), current.revision, &current.value)
                .await
            {
                Ok(_) => {
                    self.publish_best_effort(self.event(
                        "step.started",
                        &current.value.run_id,
                        Some(step_id),
                        Some(&command.worker_id),
                        &format!("generation-{}", command.lease_generation),
                        object(json!({"attempt": current.value.attempt})),
                    ))
                    .await;
                    self.refresh_run(&current.value.run_id).await?;
                    return Ok(step_mutation(&current.value));
                }
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "step start exceeded CAS retry budget".to_string(),
        ))
    }

    pub async fn heartbeat_step(
        &self,
        step_id: &str,
        command: LeaseCommand,
    ) -> Result<MutationResponse, EngineError> {
        let now = self.now_ms();
        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            validate_active_lease(&current.value, &command, now)?;
            let lease = current
                .value
                .lease
                .as_mut()
                .expect("validated active step must have a lease");
            lease.expires_at_ms = now.saturating_add(current.value.lease_ms);
            let expires_at_ms = lease.expires_at_ms;
            current.value.updated_at_ms = now;
            match self
                .update_value(&step_key(step_id), current.revision, &current.value)
                .await
            {
                Ok(_) => {
                    let worker_lane = worker_lane_key(&command.worker_id);
                    if !self
                        .extend_lane(&worker_lane, step_id, &command.lease_token, expires_at_ms)
                        .await?
                    {
                        return Err(EngineError::Conflict(
                            "worker concurrency lane was lost".to_string(),
                        ));
                    }
                    if let Some(policy) = &current.value.concurrency {
                        if !self
                            .extend_lane(
                                &concurrency_lane_key(&policy.key),
                                step_id,
                                &command.lease_token,
                                expires_at_ms,
                            )
                            .await?
                        {
                            return Err(EngineError::Conflict(
                                "keyed concurrency lane was lost".to_string(),
                            ));
                        }
                    }
                    return Ok(step_mutation(&current.value));
                }
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "step heartbeat exceeded CAS retry budget".to_string(),
        ))
    }

    pub async fn append_output(
        &self,
        step_id: &str,
        request: StepOutputRequest,
    ) -> Result<MutationResponse, EngineError> {
        validate_identifier(&request.chunk_id, "chunkId", 200)?;
        let stream = request
            .stream
            .clone()
            .unwrap_or_else(|| "output".to_string());
        validate_identifier(&stream, "stream", 80)?;
        if request.chunk.len() > MAX_OUTPUT_CHUNK_BYTES {
            return Err(EngineError::InvalidRequest(format!(
                "output chunk exceeds {MAX_OUTPUT_CHUNK_BYTES} bytes"
            )));
        }
        let final_chunk = request.final_chunk.unwrap_or(false);
        let payload_hash = stable_hash(&(stream.as_str(), request.chunk.as_str(), final_chunk))?;
        let receipt_key = output_receipt_key(step_id, &request.chunk_id);

        if let Some(existing) = self.load::<OutputReceipt>(&receipt_key).await? {
            if existing.value.payload_hash != payload_hash {
                return Err(EngineError::InvalidRequest(
                    "chunkId was already used with different output data".to_string(),
                ));
            }
            let step = self.get_step(step_id).await?;
            let event = output_event(&existing.value, &step, &request.worker_id, &request.chunk);
            self.publish_strict(event).await?;
            return Ok(step_mutation(&step));
        }

        let command = LeaseCommand {
            worker_id: request.worker_id.clone(),
            lease_token: request.lease_token.clone(),
            lease_generation: request.lease_generation,
        };
        let now = self.now_ms();
        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            validate_active_lease(&current.value, &command, now)?;
            let sequence = current.value.output_sequence.saturating_add(1);
            let event_id = stable_event_id(
                &current.value.run_id,
                Some(step_id),
                "step.output",
                &request.chunk_id,
            );
            let receipt = OutputReceipt {
                chunk_id: request.chunk_id.clone(),
                payload_hash: payload_hash.clone(),
                sequence,
                event_id,
                occurred_at_ms: now,
                stream: stream.clone(),
                final_chunk,
            };
            current.value.output_sequence = sequence;
            current.value.last_output = Some(receipt.clone());
            current.value.updated_at_ms = now;
            match self
                .update_value(&step_key(step_id), current.revision, &current.value)
                .await
            {
                Ok(_) => {
                    match self.create_value(&receipt_key, &receipt).await {
                        Ok(_) => {}
                        Err(EngineError::Store(StoreError::Conflict)) => {
                            let existing = self
                                .load::<OutputReceipt>(&receipt_key)
                                .await?
                                .ok_or_else(|| {
                                    EngineError::Conflict(
                                        "output receipt changed concurrently".to_string(),
                                    )
                                })?;
                            if existing.value.payload_hash != payload_hash {
                                return Err(EngineError::InvalidRequest(
                                    "chunkId was already used with different output data"
                                        .to_string(),
                                ));
                            }
                        }
                        Err(error) => return Err(error),
                    }
                    self.publish_strict(output_event(
                        &receipt,
                        &current.value,
                        &request.worker_id,
                        &request.chunk,
                    ))
                    .await?;
                    return Ok(step_mutation(&current.value));
                }
                Err(EngineError::Store(StoreError::Conflict)) => {
                    if let Some(existing) = self.load::<OutputReceipt>(&receipt_key).await? {
                        if existing.value.payload_hash != payload_hash {
                            return Err(EngineError::InvalidRequest(
                                "chunkId was already used with different output data".to_string(),
                            ));
                        }
                        let step = self.get_step(step_id).await?;
                        self.publish_strict(output_event(
                            &existing.value,
                            &step,
                            &request.worker_id,
                            &request.chunk,
                        ))
                        .await?;
                        return Ok(step_mutation(&step));
                    }
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "output append exceeded CAS retry budget".to_string(),
        ))
    }

    pub async fn complete_step(
        &self,
        step_id: &str,
        request: CompleteStepRequest,
    ) -> Result<MutationResponse, EngineError> {
        let expected = ExpectedLease {
            worker_id: request.worker_id.clone(),
            token: request.lease_token.clone(),
            generation: request.lease_generation,
        };
        let now = self.now_ms();
        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            if current.value.status == StepStatus::Succeeded
                && last_lease_matches(&current.value, &expected)
            {
                return Ok(step_mutation(&current.value));
            }
            validate_expected_lease(&current.value, &expected, now)?;
            let lease = current
                .value
                .lease
                .take()
                .expect("validated active step must have a lease");
            current.value.last_lease = Some(lease.clone());
            current.value.status = StepStatus::Succeeded;
            current.value.result = request.result.clone();
            current.value.failure = None;
            current.value.updated_at_ms = now;
            current.value.completed_at_ms = Some(now);
            match self
                .update_value(&step_key(step_id), current.revision, &current.value)
                .await
            {
                Ok(_) => {
                    self.release_step_lanes(&current.value, &lease).await?;
                    self.metrics
                        .step_completions_total
                        .fetch_add(1, Ordering::Relaxed);
                    self.publish_best_effort(self.event(
                        "step.succeeded",
                        &current.value.run_id,
                        Some(step_id),
                        Some(&request.worker_id),
                        &format!("generation-{}", request.lease_generation),
                        object(json!({"attempt": current.value.attempt})),
                    ))
                    .await;
                    self.advance_run_steps(&current.value.run_id).await?;
                    self.refresh_run(&current.value.run_id).await?;
                    return Ok(step_mutation(&current.value));
                }
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "step completion exceeded CAS retry budget".to_string(),
        ))
    }

    pub async fn fail_step(
        &self,
        step_id: &str,
        request: FailStepRequest,
    ) -> Result<MutationResponse, EngineError> {
        validate_identifier(&request.code, "code", 128)?;
        if request.message.trim().is_empty() || request.message.len() > 8 * 1024 {
            return Err(EngineError::InvalidRequest(
                "failure message must contain 1 to 8192 bytes".to_string(),
            ));
        }
        self.transition_failure(
            step_id,
            ExpectedLease {
                worker_id: request.worker_id,
                token: request.lease_token,
                generation: request.lease_generation,
            },
            request.code,
            request.message,
            request.retryable,
            FailureMetric::Worker,
        )
        .await
    }

    pub async fn signal_run(
        &self,
        run_id: &str,
        signal_name: &str,
        payload: JsonObject,
    ) -> Result<SignalResponse, EngineError> {
        validate_identifier(signal_name, "signalName", 128)?;
        let run = self
            .load::<RunRecord>(&run_key(run_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "run",
                id: run_id.to_string(),
            })?
            .value;
        if run.status.is_terminal() {
            return Err(EngineError::Conflict(format!("run {run_id} is terminal")));
        }

        let now = self.now_ms();
        self.store
            .put(
                &signal_key(run_id, signal_name),
                serialize(&SignalRecord {
                    payload: payload.clone(),
                    created_at_ms: now,
                })?,
            )
            .await?;

        let before = self.get_run_snapshot(run_id).await?;
        self.advance_run_steps(run_id).await?;
        let after = self.get_run_snapshot(run_id).await?;
        let before_statuses = before
            .steps
            .iter()
            .map(|step| (step.id.as_str(), step.status))
            .collect::<BTreeMap<_, _>>();
        let released_steps = after
            .steps
            .iter()
            .filter(|step| {
                step.wait_for_signal.as_deref() == Some(signal_name)
                    && before_statuses.get(step.id.as_str()) == Some(&StepStatus::WaitingSignal)
                    && step.status != StepStatus::WaitingSignal
            })
            .count() as u32;

        self.metrics.signals_total.fetch_add(1, Ordering::Relaxed);
        self.publish_best_effort(self.event(
            "run.signaled",
            run_id,
            None,
            None,
            signal_name,
            object(json!({
                "signalName": signal_name,
                "releasedSteps": released_steps,
                "payload": payload,
            })),
        ))
        .await;
        Ok(SignalResponse {
            run_id: run_id.to_string(),
            signal_name: signal_name.to_string(),
            released_steps,
        })
    }

    pub async fn pause_run(&self, run_id: &str) -> Result<MutationResponse, EngineError> {
        self.set_run_pause_state(run_id, true).await
    }

    pub async fn resume_run(&self, run_id: &str) -> Result<MutationResponse, EngineError> {
        let response = self.set_run_pause_state(run_id, false).await?;
        self.advance_run_steps(run_id).await?;
        self.refresh_run(run_id).await?;
        Ok(response)
    }

    pub async fn cancel_run(&self, run_id: &str) -> Result<MutationResponse, EngineError> {
        let current = self
            .load::<RunRecord>(&run_key(run_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "run",
                id: run_id.to_string(),
            })?;
        if current.value.status == RunStatus::Cancelled {
            return Ok(run_mutation(&current.value));
        }
        if current.value.status.is_terminal() {
            return Err(EngineError::Conflict(format!("run {run_id} is terminal")));
        }

        self.cancel_nonterminal_steps(run_id, None, "run_cancelled")
            .await?;
        self.force_run_status(run_id, RunStatus::Cancelled).await?;
        let run = self
            .load::<RunRecord>(&run_key(run_id))
            .await?
            .expect("cancelled run must still exist")
            .value;
        self.publish_best_effort(self.event(
            "run.cancelled",
            run_id,
            None,
            None,
            "cancelled",
            JsonObject::new(),
        ))
        .await;
        Ok(run_mutation(&run))
    }

    pub async fn tick(&self) -> Result<(), EngineError> {
        self.metrics
            .scheduler_ticks_total
            .fetch_add(1, Ordering::Relaxed);
        let result = self.tick_inner().await;
        if result.is_err() {
            self.metrics
                .scheduler_failures_total
                .fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    async fn tick_inner(&self) -> Result<(), EngineError> {
        let now = self.now_ms();
        let steps = self.scan_steps().await?;
        let mut runs_to_advance = BTreeSet::new();

        for step in steps {
            match step.value.status {
                StepStatus::Leased | StepStatus::Running => {
                    let Some(lease) = step.value.lease.clone() else {
                        continue;
                    };
                    let expected = ExpectedLease {
                        worker_id: lease.worker_id.clone(),
                        token: lease.token.clone(),
                        generation: lease.generation,
                    };
                    if step
                        .value
                        .started_at_ms
                        .is_some_and(|started| now >= started.saturating_add(step.value.timeout_ms))
                    {
                        let _ = self
                            .transition_failure(
                                &step.value.id,
                                expected,
                                "step_timeout".to_string(),
                                "step exceeded its hard execution timeout".to_string(),
                                true,
                                FailureMetric::Timeout,
                            )
                            .await?;
                    } else if now >= lease.expires_at_ms {
                        let _ = self
                            .transition_failure(
                                &step.value.id,
                                expected,
                                "lease_expired".to_string(),
                                "worker lease expired before completion".to_string(),
                                true,
                                FailureMetric::LeaseExpiration,
                            )
                            .await?;
                    }
                }
                StepStatus::Blocked
                | StepStatus::WaitingTimer
                | StepStatus::WaitingSignal
                | StepStatus::WaitingRetry => {
                    runs_to_advance.insert(step.value.run_id.clone());
                }
                _ => {}
            }
        }

        for run_id in runs_to_advance {
            self.advance_run_steps(&run_id).await?;
            self.refresh_run(&run_id).await?;
        }

        for key in self.store.keys().await? {
            if !key.starts_with("worker.") {
                continue;
            }
            let Some(mut current) = self.load::<WorkerRecord>(&key).await? else {
                continue;
            };
            if current.value.status != WorkerStatus::Offline
                && now.saturating_sub(current.value.last_heartbeat_ms) > current.value.ttl_ms
            {
                current.value.status = WorkerStatus::Offline;
                let _ = self
                    .update_value(&key, current.revision, &current.value)
                    .await;
            }
        }
        Ok(())
    }

    async fn try_lease_step(
        &self,
        worker: &WorkerRecord,
        step_id: &str,
    ) -> Result<Option<StepAssignment>, EngineError> {
        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            if current.value.status != StepStatus::Queued || !worker_matches(worker, &current.value)
            {
                return Ok(None);
            }
            let run = self
                .load::<RunRecord>(&run_key(&current.value.run_id))
                .await?
                .ok_or_else(|| EngineError::NotFound {
                    resource: "run",
                    id: current.value.run_id.clone(),
                })?
                .value;
            if run.status.is_terminal() || run.status == RunStatus::Paused {
                return Ok(None);
            }

            let now = self.now_ms();
            let token = Uuid::new_v4().to_string();
            let expires_at_ms = now.saturating_add(current.value.lease_ms);
            if !self
                .acquire_lane(
                    &worker_lane_key(&worker.worker_id),
                    worker.slots,
                    step_id,
                    &token,
                    expires_at_ms,
                )
                .await?
            {
                return Ok(None);
            }

            let mut concurrency_acquired = false;
            if let Some(policy) = &current.value.concurrency {
                concurrency_acquired = self
                    .acquire_lane(
                        &concurrency_lane_key(&policy.key),
                        policy.limit,
                        step_id,
                        &token,
                        expires_at_ms,
                    )
                    .await?;
                if !concurrency_acquired {
                    self.release_lane(&worker_lane_key(&worker.worker_id), step_id, &token)
                        .await?;
                    return Ok(None);
                }
            }

            current.value.attempt = current.value.attempt.saturating_add(1);
            current.value.lease_generation = current.value.lease_generation.saturating_add(1);
            let generation = current.value.lease_generation;
            current.value.lease = Some(LeaseRecord {
                token: token.clone(),
                generation,
                worker_id: worker.worker_id.clone(),
                acquired_at_ms: now,
                expires_at_ms,
                fencing_token: generation,
            });
            current.value.status = StepStatus::Leased;
            current.value.updated_at_ms = now;
            current.value.failure = None;

            match self
                .update_value(&step_key(step_id), current.revision, &current.value)
                .await
            {
                Ok(_) => {
                    self.metrics
                        .leases_granted_total
                        .fetch_add(1, Ordering::Relaxed);
                    self.publish_best_effort(self.event(
                        "step.leased",
                        &current.value.run_id,
                        Some(step_id),
                        Some(&worker.worker_id),
                        &format!("generation-{generation}"),
                        object(json!({
                            "attempt": current.value.attempt,
                            "leaseExpiresAtMs": expires_at_ms,
                            "fencingToken": generation,
                        })),
                    ))
                    .await;
                    self.refresh_run(&current.value.run_id).await?;
                    return Ok(Some(StepAssignment {
                        run_id: current.value.run_id,
                        step_id: current.value.id,
                        step_key: current.value.key,
                        task_type: current.value.task_type,
                        queue: current.value.queue,
                        input: current.value.input,
                        attempt: current.value.attempt,
                        lease_token: token,
                        lease_generation: generation,
                        fencing_token: generation,
                        lease_expires_at_ms: expires_at_ms,
                        timeout_ms: current.value.timeout_ms,
                        affinity_key: current.value.affinity_key,
                    }));
                }
                Err(EngineError::Store(StoreError::Conflict)) => {
                    self.metrics
                        .lease_conflicts_total
                        .fetch_add(1, Ordering::Relaxed);
                    self.release_lane(&worker_lane_key(&worker.worker_id), step_id, &token)
                        .await?;
                    if concurrency_acquired {
                        if let Some(policy) = &current.value.concurrency {
                            self.release_lane(&concurrency_lane_key(&policy.key), step_id, &token)
                                .await?;
                        }
                    }
                    continue;
                }
                Err(error) => {
                    self.release_lane(&worker_lane_key(&worker.worker_id), step_id, &token)
                        .await?;
                    if concurrency_acquired {
                        if let Some(policy) = &current.value.concurrency {
                            self.release_lane(&concurrency_lane_key(&policy.key), step_id, &token)
                                .await?;
                        }
                    }
                    return Err(error);
                }
            }
        }
        Ok(None)
    }

    async fn transition_failure(
        &self,
        step_id: &str,
        expected: ExpectedLease,
        code: String,
        message: String,
        retryable: bool,
        metric: FailureMetric,
    ) -> Result<MutationResponse, EngineError> {
        let now = self.now_ms();
        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            if matches!(
                current.value.status,
                StepStatus::WaitingRetry | StepStatus::Failed
            ) && last_lease_matches(&current.value, &expected)
            {
                return Ok(step_mutation(&current.value));
            }
            match metric {
                FailureMetric::Worker => {
                    validate_expected_lease(&current.value, &expected, now)?;
                }
                FailureMetric::Timeout | FailureMetric::LeaseExpiration => {
                    validate_expected_lease_identity(&current.value, &expected)?;
                }
            }
            let lease = current
                .value
                .lease
                .take()
                .expect("validated active step must have a lease");
            let should_retry =
                retryable && current.value.attempt < current.value.retry.max_attempts;
            current.value.last_lease = Some(lease.clone());
            current.value.failure = Some(FailureRecord {
                code: code.clone(),
                message: message.clone(),
                retryable,
                failed_at_ms: now,
            });
            current.value.updated_at_ms = now;
            current.value.completed_at_ms = None;
            if should_retry {
                current.value.status = StepStatus::WaitingRetry;
                current.value.not_before_ms =
                    Some(now.saturating_add(current.value.retry.backoff_ms(current.value.attempt)));
            } else {
                current.value.status = StepStatus::Failed;
                current.value.completed_at_ms = Some(now);
            }

            match self
                .update_value(&step_key(step_id), current.revision, &current.value)
                .await
            {
                Ok(_) => {
                    self.release_step_lanes(&current.value, &lease).await?;
                    self.metrics
                        .step_failures_total
                        .fetch_add(1, Ordering::Relaxed);
                    match metric {
                        FailureMetric::Worker => {}
                        FailureMetric::Timeout => {
                            self.metrics
                                .step_timeouts_total
                                .fetch_add(1, Ordering::Relaxed);
                        }
                        FailureMetric::LeaseExpiration => {
                            self.metrics
                                .lease_expirations_total
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    if should_retry {
                        self.metrics
                            .retries_scheduled_total
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    let event_type = if should_retry {
                        "step.retry_scheduled"
                    } else {
                        "step.failed"
                    };
                    self.publish_best_effort(self.event(
                        event_type,
                        &current.value.run_id,
                        Some(step_id),
                        Some(&expected.worker_id),
                        &format!("generation-{}", expected.generation),
                        object(json!({
                            "attempt": current.value.attempt,
                            "code": code,
                            "message": message,
                            "retryable": retryable,
                            "nextEligibleAtMs": current.value.not_before_ms,
                        })),
                    ))
                    .await;
                    if !should_retry {
                        self.cancel_nonterminal_steps(
                            &current.value.run_id,
                            Some(step_id),
                            "run_failed",
                        )
                        .await?;
                    }
                    self.refresh_run(&current.value.run_id).await?;
                    return Ok(step_mutation(&current.value));
                }
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "step failure transition exceeded CAS retry budget".to_string(),
        ))
    }

    async fn advance_run_steps(&self, run_id: &str) -> Result<(), EngineError> {
        let run = self
            .load::<RunRecord>(&run_key(run_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "run",
                id: run_id.to_string(),
            })?
            .value;
        if run.status.is_terminal() {
            return Ok(());
        }

        for step_id in run.step_ids.values() {
            for _ in 0..MAX_CAS_RETRIES {
                let mut current = self.load_step_versioned(step_id).await?;
                if !matches!(
                    current.value.status,
                    StepStatus::Blocked
                        | StepStatus::WaitingTimer
                        | StepStatus::WaitingSignal
                        | StepStatus::WaitingRetry
                ) {
                    break;
                }
                let now = self.now_ms();
                if current.value.status == StepStatus::WaitingRetry
                    && current
                        .value
                        .not_before_ms
                        .is_some_and(|eligible| eligible > now)
                {
                    break;
                }
                let desired = self.gate_status(&run, &current.value, now).await?;
                if desired == current.value.status {
                    break;
                }
                current.value.status = desired;
                current.value.updated_at_ms = now;
                match self
                    .update_value(&step_key(step_id), current.revision, &current.value)
                    .await
                {
                    Ok(_) => {
                        if desired == StepStatus::Queued {
                            self.publish_best_effort(self.event(
                                "step.queued",
                                run_id,
                                Some(step_id),
                                None,
                                &format!("attempt-{}", current.value.attempt.saturating_add(1)),
                                object(json!({"stepKey": current.value.key})),
                            ))
                            .await;
                        }
                        break;
                    }
                    Err(EngineError::Store(StoreError::Conflict)) => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    async fn gate_status(
        &self,
        run: &RunRecord,
        step: &StepRecord,
        now: u64,
    ) -> Result<StepStatus, EngineError> {
        for dependency_key in &step.depends_on {
            let dependency_id = run.step_ids.get(dependency_key).ok_or_else(|| {
                EngineError::InvalidRequest(format!(
                    "step {} references missing dependency {dependency_key}",
                    step.key
                ))
            })?;
            let dependency = self.get_step(dependency_id).await?;
            if dependency.status != StepStatus::Succeeded {
                return Ok(StepStatus::Blocked);
            }
        }
        if step.not_before_ms.is_some_and(|eligible| eligible > now) {
            return Ok(StepStatus::WaitingTimer);
        }
        if let Some(signal_name) = step.wait_for_signal.as_deref() {
            if self
                .store
                .get(&signal_key(&step.run_id, signal_name))
                .await?
                .is_none()
            {
                return Ok(StepStatus::WaitingSignal);
            }
        }
        Ok(StepStatus::Queued)
    }

    async fn refresh_run(&self, run_id: &str) -> Result<RunRecord, EngineError> {
        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self
                .load::<RunRecord>(&run_key(run_id))
                .await?
                .ok_or_else(|| EngineError::NotFound {
                    resource: "run",
                    id: run_id.to_string(),
                })?;
            let mut steps = Vec::with_capacity(current.value.step_ids.len());
            for step_id in current.value.step_ids.values() {
                steps.push(self.get_step(step_id).await?);
            }
            let counts = counts_for_steps(&steps);
            let previous_status = current.value.status;
            let desired_status = desired_run_status(previous_status, &counts);
            if current.value.counts.total == counts.total
                && current.value.counts.blocked == counts.blocked
                && current.value.counts.queued == counts.queued
                && current.value.counts.active == counts.active
                && current.value.counts.succeeded == counts.succeeded
                && current.value.counts.failed == counts.failed
                && current.value.counts.cancelled == counts.cancelled
                && previous_status == desired_status
            {
                return Ok(current.value);
            }

            let now = self.now_ms();
            current.value.counts = counts;
            current.value.status = desired_status;
            current.value.updated_at_ms = now;
            current.value.completed_at_ms = desired_status.is_terminal().then_some(now);
            match self
                .update_value(&run_key(run_id), current.revision, &current.value)
                .await
            {
                Ok(_) => {
                    if previous_status != desired_status {
                        self.publish_best_effort(self.event(
                            run_event_type(desired_status),
                            run_id,
                            None,
                            None,
                            run_status_name(desired_status),
                            object(json!({"status": run_status_name(desired_status)})),
                        ))
                        .await;
                    }
                    return Ok(current.value);
                }
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "run refresh exceeded CAS retry budget".to_string(),
        ))
    }

    async fn set_run_pause_state(
        &self,
        run_id: &str,
        paused: bool,
    ) -> Result<MutationResponse, EngineError> {
        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self
                .load::<RunRecord>(&run_key(run_id))
                .await?
                .ok_or_else(|| EngineError::NotFound {
                    resource: "run",
                    id: run_id.to_string(),
                })?;
            if current.value.status.is_terminal() {
                return Err(EngineError::Conflict(format!("run {run_id} is terminal")));
            }
            if paused && current.value.status == RunStatus::Paused {
                return Ok(run_mutation(&current.value));
            }
            if !paused && current.value.status != RunStatus::Paused {
                return Ok(run_mutation(&current.value));
            }
            current.value.status = if paused {
                RunStatus::Paused
            } else if current.value.counts.active > 0 || current.value.counts.succeeded > 0 {
                RunStatus::Running
            } else {
                RunStatus::Pending
            };
            current.value.updated_at_ms = self.now_ms();
            match self
                .update_value(&run_key(run_id), current.revision, &current.value)
                .await
            {
                Ok(_) => {
                    self.publish_best_effort(self.event(
                        if paused { "run.paused" } else { "run.resumed" },
                        run_id,
                        None,
                        None,
                        if paused { "paused" } else { "resumed" },
                        JsonObject::new(),
                    ))
                    .await;
                    return Ok(run_mutation(&current.value));
                }
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "run pause transition exceeded CAS retry budget".to_string(),
        ))
    }

    async fn force_run_status(&self, run_id: &str, status: RunStatus) -> Result<(), EngineError> {
        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self
                .load::<RunRecord>(&run_key(run_id))
                .await?
                .ok_or_else(|| EngineError::NotFound {
                    resource: "run",
                    id: run_id.to_string(),
                })?;
            let snapshot = self.get_run_snapshot(run_id).await?;
            current.value.counts = counts_for_steps(&snapshot.steps);
            current.value.status = status;
            current.value.updated_at_ms = self.now_ms();
            current.value.completed_at_ms = status.is_terminal().then_some(self.now_ms());
            match self
                .update_value(&run_key(run_id), current.revision, &current.value)
                .await
            {
                Ok(_) => return Ok(()),
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "run status update exceeded CAS retry budget".to_string(),
        ))
    }

    async fn cancel_nonterminal_steps(
        &self,
        run_id: &str,
        except_step_id: Option<&str>,
        reason: &str,
    ) -> Result<(), EngineError> {
        let run = self
            .load::<RunRecord>(&run_key(run_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "run",
                id: run_id.to_string(),
            })?
            .value;
        for step_id in run.step_ids.values() {
            if except_step_id == Some(step_id.as_str()) {
                continue;
            }
            for _ in 0..MAX_CAS_RETRIES {
                let mut current = self.load_step_versioned(step_id).await?;
                if current.value.status.is_terminal() {
                    break;
                }
                let lease = current.value.lease.take();
                if let Some(lease) = &lease {
                    current.value.last_lease = Some(lease.clone());
                }
                current.value.status = StepStatus::Cancelled;
                current.value.updated_at_ms = self.now_ms();
                current.value.completed_at_ms = Some(self.now_ms());
                match self
                    .update_value(&step_key(step_id), current.revision, &current.value)
                    .await
                {
                    Ok(_) => {
                        if let Some(lease) = lease {
                            self.release_step_lanes(&current.value, &lease).await?;
                        }
                        self.publish_best_effort(self.event(
                            "step.cancelled",
                            run_id,
                            Some(step_id),
                            None,
                            reason,
                            object(json!({"reason": reason})),
                        ))
                        .await;
                        break;
                    }
                    Err(EngineError::Store(StoreError::Conflict)) => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    async fn acquire_lane(
        &self,
        key: &str,
        limit: u32,
        holder_id: &str,
        lease_token: &str,
        expires_at_ms: u64,
    ) -> Result<bool, EngineError> {
        for _ in 0..MAX_CAS_RETRIES {
            let now = self.now_ms();
            let current = self.load::<LaneRecord>(key).await?;
            let mut lane = current
                .as_ref()
                .map(|record| record.value.clone())
                .unwrap_or_else(|| LaneRecord {
                    limit,
                    holders: BTreeMap::new(),
                    updated_at_ms: now,
                });
            lane.limit = limit.max(1);
            lane.holders.retain(|_, holder| holder.expires_at_ms > now);
            if let Some(holder) = lane.holders.get_mut(holder_id) {
                if holder.lease_token != lease_token {
                    return Ok(false);
                }
                holder.expires_at_ms = expires_at_ms;
            } else {
                if lane.holders.len() >= lane.limit as usize {
                    return Ok(false);
                }
                lane.holders.insert(
                    holder_id.to_string(),
                    LaneHolder {
                        lease_token: lease_token.to_string(),
                        expires_at_ms,
                    },
                );
            }
            lane.updated_at_ms = now;
            let result = if let Some(current) = current {
                self.update_value(key, current.revision, &lane).await
            } else {
                self.create_value(key, &lane).await
            };
            match result {
                Ok(_) => return Ok(true),
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "lane acquisition exceeded CAS retry budget".to_string(),
        ))
    }

    async fn extend_lane(
        &self,
        key: &str,
        holder_id: &str,
        lease_token: &str,
        expires_at_ms: u64,
    ) -> Result<bool, EngineError> {
        for _ in 0..MAX_CAS_RETRIES {
            let Some(mut current) = self.load::<LaneRecord>(key).await? else {
                return Ok(false);
            };
            let now = self.now_ms();
            current
                .value
                .holders
                .retain(|_, holder| holder.expires_at_ms > now);
            let Some(holder) = current.value.holders.get_mut(holder_id) else {
                return Ok(false);
            };
            if holder.lease_token != lease_token {
                return Ok(false);
            }
            holder.expires_at_ms = expires_at_ms;
            current.value.updated_at_ms = now;
            match self
                .update_value(key, current.revision, &current.value)
                .await
            {
                Ok(_) => return Ok(true),
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "lane extension exceeded CAS retry budget".to_string(),
        ))
    }

    async fn release_lane(
        &self,
        key: &str,
        holder_id: &str,
        lease_token: &str,
    ) -> Result<(), EngineError> {
        for _ in 0..MAX_CAS_RETRIES {
            let Some(mut current) = self.load::<LaneRecord>(key).await? else {
                return Ok(());
            };
            let should_remove = current
                .value
                .holders
                .get(holder_id)
                .is_some_and(|holder| holder.lease_token == lease_token);
            if !should_remove {
                return Ok(());
            }
            current.value.holders.remove(holder_id);
            current.value.updated_at_ms = self.now_ms();
            match self
                .update_value(key, current.revision, &current.value)
                .await
            {
                Ok(_) => return Ok(()),
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "lane release exceeded CAS retry budget".to_string(),
        ))
    }

    async fn release_step_lanes(
        &self,
        step: &StepRecord,
        lease: &LeaseRecord,
    ) -> Result<(), EngineError> {
        self.release_lane(&worker_lane_key(&lease.worker_id), &step.id, &lease.token)
            .await?;
        if let Some(policy) = &step.concurrency {
            self.release_lane(&concurrency_lane_key(&policy.key), &step.id, &lease.token)
                .await?;
        }
        Ok(())
    }

    async fn idempotent_submit_response(
        &self,
        run_id: &str,
    ) -> Result<SubmitRunResponse, EngineError> {
        let run = self
            .load::<RunRecord>(&run_key(run_id))
            .await?
            .ok_or_else(|| {
                EngineError::Conflict(format!(
                    "idempotency record points to an incomplete run {run_id}"
                ))
            })?
            .value;
        Ok(SubmitRunResponse {
            run_id: run_id.to_string(),
            status: run.status,
            idempotent_replay: true,
        })
    }

    async fn load_step_versioned(
        &self,
        step_id: &str,
    ) -> Result<Versioned<StepRecord>, EngineError> {
        self.load::<StepRecord>(&step_key(step_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "step",
                id: step_id.to_string(),
            })
    }

    async fn scan_steps(&self) -> Result<Vec<Versioned<StepRecord>>, EngineError> {
        let mut result = Vec::new();
        for key in self.store.keys().await? {
            if !key.starts_with("step.") {
                continue;
            }
            if let Some(value) = self.load::<StepRecord>(&key).await? {
                result.push(value);
            }
        }
        Ok(result)
    }

    async fn load<T: DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<Versioned<T>>, EngineError> {
        self.store
            .get(key)
            .await?
            .map(deserialize)
            .transpose()
            .map_err(EngineError::from)
    }

    async fn create_value<T: Serialize>(&self, key: &str, value: &T) -> Result<u64, EngineError> {
        Ok(self.store.create(key, serialize(value)?).await?)
    }

    async fn update_value<T: Serialize>(
        &self,
        key: &str,
        revision: u64,
        value: &T,
    ) -> Result<u64, EngineError> {
        Ok(self.store.update(key, revision, serialize(value)?).await?)
    }

    fn event(
        &self,
        event_type: &str,
        run_id: &str,
        step_id: Option<&str>,
        worker_id: Option<&str>,
        discriminator: &str,
        data: JsonObject,
    ) -> DurableEvent {
        DurableEvent {
            schema_version: "1".to_string(),
            event_id: stable_event_id(run_id, step_id, event_type, discriminator),
            event_type: event_type.to_string(),
            run_id: run_id.to_string(),
            step_id: step_id.map(ToString::to_string),
            worker_id: worker_id.map(ToString::to_string),
            occurred_at_ms: self.now_ms(),
            data,
        }
    }

    async fn publish_best_effort(&self, event: DurableEvent) {
        if self.events.publish(&event).await.is_err() {
            self.metrics
                .journal_failures_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    async fn publish_strict(&self, event: DurableEvent) -> Result<(), EngineError> {
        self.events.publish(&event).await?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum FailureMetric {
    Worker,
    Timeout,
    LeaseExpiration,
}

fn initial_step_status(definition: &crate::model::StepDefinition, now: u64) -> StepStatus {
    if !definition.depends_on.is_empty() {
        StepStatus::Blocked
    } else if definition
        .not_before_ms
        .is_some_and(|eligible| eligible > now)
    {
        StepStatus::WaitingTimer
    } else if definition.wait_for_signal.is_some() {
        StepStatus::WaitingSignal
    } else {
        StepStatus::Queued
    }
}

fn desired_run_status(previous: RunStatus, counts: &RunCounts) -> RunStatus {
    if counts.failed > 0 {
        RunStatus::Failed
    } else if counts.total > 0 && counts.succeeded == counts.total {
        RunStatus::Succeeded
    } else if counts.total > 0
        && counts.succeeded + counts.failed + counts.cancelled == counts.total
        && counts.cancelled > 0
    {
        RunStatus::Cancelled
    } else if previous == RunStatus::Paused {
        RunStatus::Paused
    } else if counts.active > 0 || counts.succeeded > 0 {
        RunStatus::Running
    } else {
        RunStatus::Pending
    }
}

fn counts_for_steps(steps: &[StepRecord]) -> RunCounts {
    let mut counts = RunCounts {
        total: steps.len() as u32,
        blocked: 0,
        queued: 0,
        active: 0,
        succeeded: 0,
        failed: 0,
        cancelled: 0,
    };
    for step in steps {
        match step.status {
            StepStatus::Blocked
            | StepStatus::WaitingTimer
            | StepStatus::WaitingSignal
            | StepStatus::WaitingRetry => counts.blocked += 1,
            StepStatus::Queued => counts.queued += 1,
            StepStatus::Leased | StepStatus::Running => counts.active += 1,
            StepStatus::Succeeded => counts.succeeded += 1,
            StepStatus::Failed => counts.failed += 1,
            StepStatus::Cancelled => counts.cancelled += 1,
        }
    }
    counts
}

fn worker_matches(worker: &WorkerRecord, step: &StepRecord) -> bool {
    worker.queues.contains(&step.queue)
        && step
            .required_capabilities
            .iter()
            .all(|capability| worker.capabilities.contains(capability))
        && step.affinity_key.as_deref().is_none_or(|affinity| {
            worker.worker_id == affinity
                || worker
                    .labels
                    .get("affinity")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == affinity)
                || worker
                    .labels
                    .get("affinityKey")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == affinity)
        })
}

fn validate_active_lease(
    step: &StepRecord,
    command: &LeaseCommand,
    now: u64,
) -> Result<(), EngineError> {
    validate_expected_lease(
        step,
        &ExpectedLease {
            worker_id: command.worker_id.clone(),
            token: command.lease_token.clone(),
            generation: command.lease_generation,
        },
        now,
    )
}

fn validate_expected_lease_identity(
    step: &StepRecord,
    expected: &ExpectedLease,
) -> Result<(), EngineError> {
    if !step.status.is_active() {
        return Err(EngineError::Conflict(format!(
            "step {} has no active lease",
            step.id
        )));
    }
    let lease = step.lease.as_ref().ok_or_else(|| {
        EngineError::Conflict(format!("step {} has no active lease record", step.id))
    })?;
    if lease.worker_id != expected.worker_id
        || lease.token != expected.token
        || lease.generation != expected.generation
    {
        return Err(EngineError::Conflict(format!(
            "stale lease for step {}",
            step.id
        )));
    }
    Ok(())
}

fn validate_expected_lease(
    step: &StepRecord,
    expected: &ExpectedLease,
    now: u64,
) -> Result<(), EngineError> {
    validate_expected_lease_identity(step, expected)?;
    let lease = step
        .lease
        .as_ref()
        .expect("validated active step must have a lease");
    if now >= lease.expires_at_ms {
        return Err(EngineError::Conflict(format!(
            "lease for step {} has expired",
            step.id
        )));
    }
    Ok(())
}

fn current_lease_matches(step: &StepRecord, command: &LeaseCommand) -> bool {
    step.lease.as_ref().is_some_and(|lease| {
        lease.worker_id == command.worker_id
            && lease.token == command.lease_token
            && lease.generation == command.lease_generation
    })
}

fn last_lease_matches(step: &StepRecord, expected: &ExpectedLease) -> bool {
    step.last_lease.as_ref().is_some_and(|lease| {
        lease.worker_id == expected.worker_id
            && lease.token == expected.token
            && lease.generation == expected.generation
    })
}

fn step_mutation(step: &StepRecord) -> MutationResponse {
    MutationResponse {
        ok: true,
        run_id: Some(step.run_id.clone()),
        step_id: Some(step.id.clone()),
        status: Some(step_status_name(step.status).to_string()),
    }
}

fn run_mutation(run: &RunRecord) -> MutationResponse {
    MutationResponse {
        ok: true,
        run_id: Some(run.id.clone()),
        step_id: None,
        status: Some(run_status_name(run.status).to_string()),
    }
}

fn step_status_name(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Blocked => "blocked",
        StepStatus::WaitingTimer => "waiting_timer",
        StepStatus::WaitingSignal => "waiting_signal",
        StepStatus::Queued => "queued",
        StepStatus::Leased => "leased",
        StepStatus::Running => "running",
        StepStatus::WaitingRetry => "waiting_retry",
        StepStatus::Succeeded => "succeeded",
        StepStatus::Failed => "failed",
        StepStatus::Cancelled => "cancelled",
    }
}

fn run_status_name(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Pending => "pending",
        RunStatus::Running => "running",
        RunStatus::Paused => "paused",
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn run_event_type(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Pending => "run.pending",
        RunStatus::Running => "run.running",
        RunStatus::Paused => "run.paused",
        RunStatus::Succeeded => "run.succeeded",
        RunStatus::Failed => "run.failed",
        RunStatus::Cancelled => "run.cancelled",
    }
}

fn stable_event_id(
    run_id: &str,
    step_id: Option<&str>,
    event_type: &str,
    discriminator: &str,
) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!(
            "{run_id}|{}|{event_type}|{discriminator}",
            step_id.unwrap_or_default()
        )
        .as_bytes(),
    )
    .to_string()
}

fn output_event(
    receipt: &OutputReceipt,
    step: &StepRecord,
    worker_id: &str,
    chunk: &str,
) -> DurableEvent {
    DurableEvent {
        schema_version: "1".to_string(),
        event_id: receipt.event_id.clone(),
        event_type: "step.output".to_string(),
        run_id: step.run_id.clone(),
        step_id: Some(step.id.clone()),
        worker_id: Some(worker_id.to_string()),
        occurred_at_ms: receipt.occurred_at_ms,
        data: object(json!({
            "chunkId": receipt.chunk_id,
            "sequence": receipt.sequence,
            "stream": receipt.stream,
            "chunk": chunk,
            "finalChunk": receipt.final_chunk,
        })),
    }
}

fn object(value: Value) -> JsonObject {
    serde_json::from_value(value).expect("JSON object literal must deserialize")
}

fn stable_hash<T: Serialize>(value: &T) -> Result<String, EngineError> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| StoreError::Serialization(error.to_string()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn serialize<T: Serialize>(value: &T) -> Result<Bytes, StoreError> {
    serde_json::to_vec(value)
        .map(Bytes::from)
        .map_err(|error| StoreError::Serialization(error.to_string()))
}

fn deserialize<T: DeserializeOwned>(stored: StoredValue) -> Result<Versioned<T>, StoreError> {
    let value = serde_json::from_slice(&stored.value)
        .map_err(|error| StoreError::Serialization(error.to_string()))?;
    Ok(Versioned {
        revision: stored.revision,
        value,
    })
}

fn hash_key(prefix: &str, value: &str) -> String {
    format!("{prefix}.{}", hex::encode(Sha256::digest(value.as_bytes())))
}

fn run_key(run_id: &str) -> String {
    format!("run.{run_id}")
}

fn step_key(step_id: &str) -> String {
    format!("step.{step_id}")
}

fn worker_key(worker_id: &str) -> String {
    hash_key("worker", worker_id)
}

fn idempotency_key_key(idempotency_key: &str) -> String {
    hash_key("idempotency", idempotency_key)
}

fn signal_key(run_id: &str, signal_name: &str) -> String {
    hash_key("signal", &format!("{run_id}:{signal_name}"))
}

fn worker_lane_key(worker_id: &str) -> String {
    hash_key("lane.worker", worker_id)
}

fn concurrency_lane_key(concurrency_key: &str) -> String {
    hash_key("lane.concurrency", concurrency_key)
}

fn output_receipt_key(step_id: &str, chunk_id: &str) -> String {
    hash_key("output", &format!("{step_id}:{chunk_id}"))
}
