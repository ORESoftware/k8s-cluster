use crate::client::{
    Assignment, Client, JsonObject, Lease, StepCompletion, StepFailure, StepOutput,
    WorkerPoll, WorkerRegistration,
};
use crate::error::{DurableWorkerError, ProtocolError};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinSet;

pub type WorkerFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DurableWorkerError>> + Send + 'a>>;
pub type HandlerFuture =
    Pin<Box<dyn Future<Output = Result<JsonObject, WorkerFailure>> + Send + 'static>>;
pub type Handler = Arc<dyn Fn(TaskContext) -> HandlerFuture + Send + Sync>;

pub trait WorkerApi: Send + Sync {
    fn register_worker<'a>(
        &'a self,
        registration: WorkerRegistration,
    ) -> WorkerFuture<'a, ()>;
    fn heartbeat_worker<'a>(
        &'a self,
        worker_id: &'a str,
        drain: Option<bool>,
    ) -> WorkerFuture<'a, ()>;
    fn poll_worker<'a>(&'a self, worker_id: &'a str, wait_ms: u64)
        -> WorkerFuture<'a, WorkerPoll>;
    fn start_step<'a>(&'a self, step_id: &'a str, lease: Lease) -> WorkerFuture<'a, ()>;
    fn heartbeat_step<'a>(
        &'a self,
        step_id: &'a str,
        lease: Lease,
    ) -> WorkerFuture<'a, ()>;
    fn append_step_output<'a>(
        &'a self,
        step_id: &'a str,
        output: StepOutput,
    ) -> WorkerFuture<'a, ()>;
    fn complete_step<'a>(
        &'a self,
        step_id: &'a str,
        completion: StepCompletion,
    ) -> WorkerFuture<'a, ()>;
    fn fail_step<'a>(
        &'a self,
        step_id: &'a str,
        failure: StepFailure,
    ) -> WorkerFuture<'a, ()>;
}

impl WorkerApi for Client {
    fn register_worker<'a>(
        &'a self,
        registration: WorkerRegistration,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move { Client::register_worker(self, registration).await.map(|_| ()) })
    }

    fn heartbeat_worker<'a>(
        &'a self,
        worker_id: &'a str,
        drain: Option<bool>,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move { Client::heartbeat_worker(self, worker_id, drain).await.map(|_| ()) })
    }

    fn poll_worker<'a>(
        &'a self,
        worker_id: &'a str,
        wait_ms: u64,
    ) -> WorkerFuture<'a, WorkerPoll> {
        Box::pin(async move { Client::poll_worker(self, worker_id, wait_ms).await })
    }

    fn start_step<'a>(&'a self, step_id: &'a str, lease: Lease) -> WorkerFuture<'a, ()> {
        Box::pin(async move { Client::start_step(self, step_id, lease).await.map(|_| ()) })
    }

    fn heartbeat_step<'a>(
        &'a self,
        step_id: &'a str,
        lease: Lease,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move { Client::heartbeat_step(self, step_id, lease).await.map(|_| ()) })
    }

    fn append_step_output<'a>(
        &'a self,
        step_id: &'a str,
        output: StepOutput,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            Client::append_step_output(self, step_id, output)
                .await
                .map(|_| ())
        })
    }

    fn complete_step<'a>(
        &'a self,
        step_id: &'a str,
        completion: StepCompletion,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            Client::complete_step(self, step_id, completion)
                .await
                .map(|_| ())
        })
    }

    fn fail_step<'a>(
        &'a self,
        step_id: &'a str,
        failure: StepFailure,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move { Client::fail_step(self, step_id, failure).await.map(|_| ()) })
    }
}

#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub worker_id: String,
    pub queues: Vec<String>,
    pub capabilities: Vec<String>,
    pub labels: JsonObject,
    pub slots: usize,
    pub ttl_ms: u64,
    pub poll_wait_ms: u64,
    pub worker_heartbeat_ms: u64,
    pub step_heartbeat_ms: u64,
    pub max_assignments: Option<usize>,
    pub idle_sleep_ms: u64,
}

impl WorkerConfig {
    pub fn validate(&self) -> Result<(), DurableWorkerError> {
        if self.worker_id.is_empty() {
            return Err(DurableWorkerError::Configuration(
                "worker ID must be non-empty".to_owned(),
            ));
        }
        if self.queues.is_empty() || self.queues.iter().any(String::is_empty) {
            return Err(DurableWorkerError::Configuration(
                "at least one non-empty queue is required".to_owned(),
            ));
        }
        if self.slots == 0 {
            return Err(DurableWorkerError::Configuration(
                "worker slots must be positive".to_owned(),
            ));
        }
        if self.ttl_ms == 0
            || self.worker_heartbeat_ms == 0
            || self.step_heartbeat_ms == 0
        {
            return Err(DurableWorkerError::Configuration(
                "worker TTL and heartbeat intervals must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: "rust-worker".to_owned(),
            queues: vec!["default".to_owned()],
            capabilities: Vec::new(),
            labels: JsonObject::new(),
            slots: 1,
            ttl_ms: 45_000,
            poll_wait_ms: 30_000,
            worker_heartbeat_ms: 15_000,
            step_heartbeat_ms: 15_000,
            max_assignments: None,
            idle_sleep_ms: 100,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkerSummary {
    pub accepted: usize,
    pub completed: usize,
    pub failed: usize,
    pub lease_lost: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl WorkerFailure {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }
}

impl From<DurableWorkerError> for WorkerFailure {
    fn from(error: DurableWorkerError) -> Self {
        let code = if error.is_lease_lost() {
            "lease_lost"
        } else {
            "sdk_error"
        };
        Self::new(code, error.to_string(), error.retryable())
    }
}

#[derive(Clone, Default)]
pub struct Cancellation {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl Cancellation {
    pub fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    pub fn check(&self) -> Result<(), DurableWorkerError> {
        if self.is_cancelled() {
            Err(DurableWorkerError::LeaseLost(ProtocolError::new(
                "lease_lost",
                "task lease is no longer authoritative",
                None,
                false,
            )))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
pub struct TaskContext {
    api: Arc<dyn WorkerApi>,
    pub assignment: Assignment,
    lease: Lease,
    cancellation: Cancellation,
    output_sequence: Arc<AtomicU64>,
}

impl TaskContext {
    fn new(
        api: Arc<dyn WorkerApi>,
        assignment: Assignment,
        lease: Lease,
        cancellation: Cancellation,
    ) -> Self {
        Self {
            api,
            assignment,
            lease,
            cancellation,
            output_sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn input(&self) -> &JsonObject {
        &self.assignment.input
    }

    pub fn fencing_token(&self) -> i64 {
        self.assignment.fencing_token
    }

    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    pub fn check_cancelled(&self) -> Result<(), DurableWorkerError> {
        self.cancellation.check()
    }

    pub async fn emit(
        &self,
        chunk: impl Into<String>,
        stream: impl Into<String>,
        final_chunk: bool,
    ) -> Result<(), DurableWorkerError> {
        self.check_cancelled()?;
        let sequence = self.output_sequence.fetch_add(1, Ordering::AcqRel) + 1;
        let chunk_id = format!(
            "{}:{}:{}",
            self.assignment.step_id, self.lease.lease_generation, sequence
        );
        let result = self
            .api
            .append_step_output(
                &self.assignment.step_id,
                StepOutput {
                    lease: self.lease.clone(),
                    chunk_id,
                    chunk: chunk.into(),
                    stream: stream.into(),
                    final_chunk,
                },
            )
            .await;
        if matches!(&result, Err(error) if error.is_lease_lost()) {
            self.cancellation.cancel();
        }
        result
    }
}

pub struct Worker {
    api: Arc<dyn WorkerApi>,
    handlers: HashMap<String, Handler>,
    config: WorkerConfig,
}

impl Worker {
    pub fn new(
        api: Arc<dyn WorkerApi>,
        handlers: HashMap<String, Handler>,
        config: WorkerConfig,
    ) -> Result<Self, DurableWorkerError> {
        config.validate()?;
        Ok(Self {
            api,
            handlers,
            config,
        })
    }

    pub async fn run(
        &self,
        shutdown: Cancellation,
    ) -> Result<WorkerSummary, DurableWorkerError> {
        self.api
            .register_worker(WorkerRegistration {
                worker_id: self.config.worker_id.clone(),
                queues: self.config.queues.clone(),
                capabilities: self.config.capabilities.clone(),
                labels: self.config.labels.clone(),
                slots: self.config.slots,
                ttl_ms: self.config.ttl_ms,
                drain: false,
            })
            .await?;

        let heartbeat_stop = Cancellation::default();
        let heartbeat_handle = tokio::spawn(worker_heartbeat_loop(
            Arc::clone(&self.api),
            self.config.worker_id.clone(),
            self.config.worker_heartbeat_ms,
            heartbeat_stop.clone(),
            shutdown.clone(),
        ));

        let mut tasks = JoinSet::new();
        let mut summary = WorkerSummary::default();
        let run_result = async {
            loop {
                while let Some(joined) = tasks.try_join_next() {
                    let outcome = joined.map_err(|error| {
                        DurableWorkerError::WorkerJoin(error.to_string())
                    })?;
                    apply_outcome(&mut summary, outcome);
                }

                let limit_reached = self
                    .config
                    .max_assignments
                    .is_some_and(|limit| summary.accepted >= limit);
                if (shutdown.is_cancelled() || limit_reached) && tasks.is_empty() {
                    break;
                }
                if shutdown.is_cancelled() || limit_reached || tasks.len() >= self.config.slots {
                    if let Some(joined) = tasks.join_next().await {
                        let outcome = joined.map_err(|error| {
                            DurableWorkerError::WorkerJoin(error.to_string())
                        })?;
                        apply_outcome(&mut summary, outcome);
                    }
                    continue;
                }

                let poll = self
                    .api
                    .poll_worker(&self.config.worker_id, self.config.poll_wait_ms)
                    .await?;
                let Some(assignment) = poll.assignment else {
                    let delay = if poll.retry_after_ms == 0 {
                        self.config.idle_sleep_ms
                    } else {
                        poll.retry_after_ms
                    };
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                };

                summary.accepted += 1;
                let handler = self.handlers.get(&assignment.task_type).cloned();
                tasks.spawn(execute_assignment(
                    Arc::clone(&self.api),
                    self.config.worker_id.clone(),
                    assignment,
                    handler,
                    self.config.step_heartbeat_ms,
                ));
            }
            Ok::<(), DurableWorkerError>(())
        }
        .await;

        heartbeat_stop.cancel();
        let heartbeat_result = heartbeat_handle
            .await
            .map_err(|error| DurableWorkerError::WorkerJoin(error.to_string()))?;
        let _ = self
            .api
            .heartbeat_worker(&self.config.worker_id, Some(true))
            .await;

        run_result?;
        heartbeat_result?;
        Ok(summary)
    }
}

async fn worker_heartbeat_loop(
    api: Arc<dyn WorkerApi>,
    worker_id: String,
    interval_ms: u64,
    stop: Cancellation,
    shutdown: Cancellation,
) -> Result<(), DurableWorkerError> {
    let interval = Duration::from_millis(interval_ms);
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = tokio::time::sleep(interval) => {
                if let Err(error) = api
                    .heartbeat_worker(&worker_id, Some(shutdown.is_cancelled()))
                    .await
                {
                    if !error.retryable() {
                        shutdown.cancel();
                        return Err(error);
                    }
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum TaskOutcome {
    Completed,
    Failed,
    LeaseLost,
}

fn apply_outcome(summary: &mut WorkerSummary, outcome: TaskOutcome) {
    match outcome {
        TaskOutcome::Completed => summary.completed += 1,
        TaskOutcome::Failed => summary.failed += 1,
        TaskOutcome::LeaseLost => summary.lease_lost += 1,
    }
}

async fn execute_assignment(
    api: Arc<dyn WorkerApi>,
    worker_id: String,
    assignment: Assignment,
    handler: Option<Handler>,
    heartbeat_ms: u64,
) -> TaskOutcome {
    if assignment.step_id.is_empty() || assignment.task_type.is_empty() {
        return TaskOutcome::Failed;
    }
    let lease = assignment.lease(worker_id);
    if let Err(error) = api.start_step(&assignment.step_id, lease.clone()).await {
        return if error.is_lease_lost() {
            TaskOutcome::LeaseLost
        } else {
            TaskOutcome::Failed
        };
    }

    let cancellation = Cancellation::default();
    let heartbeat_stop = Cancellation::default();
    let heartbeat_handle = tokio::spawn(step_heartbeat_loop(
        Arc::clone(&api),
        assignment.step_id.clone(),
        lease.clone(),
        heartbeat_ms,
        heartbeat_stop.clone(),
        cancellation.clone(),
    ));
    let context = TaskContext::new(
        Arc::clone(&api),
        assignment.clone(),
        lease.clone(),
        cancellation.clone(),
    );

    let result = match handler {
        Some(handler) => handler(context).await,
        None => Err(WorkerFailure::new(
            "handler_not_found",
            format!("no handler registered for task type {}", assignment.task_type),
            false,
        )),
    };

    heartbeat_stop.cancel();
    if heartbeat_handle.await.is_err() {
        cancellation.cancel();
    }
    if cancellation.is_cancelled() {
        return TaskOutcome::LeaseLost;
    }

    match result {
        Ok(result) => match api
            .complete_step(
                &assignment.step_id,
                StepCompletion {
                    lease,
                    result,
                },
            )
            .await
        {
            Ok(()) => TaskOutcome::Completed,
            Err(error) if error.is_lease_lost() => TaskOutcome::LeaseLost,
            Err(_) => TaskOutcome::Failed,
        },
        Err(failure) => match api
            .fail_step(
                &assignment.step_id,
                StepFailure {
                    lease,
                    code: failure.code,
                    message: failure.message,
                    retryable: failure.retryable,
                },
            )
            .await
        {
            Ok(()) => TaskOutcome::Failed,
            Err(error) if error.is_lease_lost() => TaskOutcome::LeaseLost,
            Err(_) => TaskOutcome::Failed,
        },
    }
}

async fn step_heartbeat_loop(
    api: Arc<dyn WorkerApi>,
    step_id: String,
    lease: Lease,
    interval_ms: u64,
    stop: Cancellation,
    cancellation: Cancellation,
) {
    let interval = Duration::from_millis(interval_ms);
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = tokio::time::sleep(interval) => {
                if api
                    .heartbeat_step(&step_id, lease.clone())
                    .await
                    .is_err()
                {
                    cancellation.cancel();
                    break;
                }
            }
        }
    }
}
