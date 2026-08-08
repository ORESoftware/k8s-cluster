use oresoftware_durable_worker::{
    Assignment, Cancellation, DurableWorkerError, Handler, JsonObject, Lease, StepCompletion,
    StepFailure, StepOutput, TaskContext, Worker, WorkerApi, WorkerConfig, WorkerFailure,
    WorkerFuture, WorkerPoll, WorkerRegistration,
};
use serde_json::json;
use std::collections::HashMap;
use std::future::pending;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone)]
struct FakeApi {
    calls: Arc<Mutex<Vec<String>>>,
    assignment: Arc<Mutex<Option<Assignment>>>,
    fence_heartbeat: Arc<AtomicBool>,
    fence_output: Arc<AtomicBool>,
    block_poll: Arc<AtomicBool>,
    block_step_heartbeat: Arc<AtomicBool>,
    complete_protocol_error: Arc<AtomicBool>,
    poll_observed: Arc<Notify>,
    heartbeat_observed: Arc<Notify>,
    step_heartbeat_count: Arc<AtomicUsize>,
    output_chunk_ids: Arc<Mutex<Vec<String>>>,
    failure: Arc<Mutex<Option<StepFailure>>>,
    completion: Arc<Mutex<Option<StepCompletion>>>,
}

impl Default for FakeApi {
    fn default() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            assignment: Arc::new(Mutex::new(None)),
            fence_heartbeat: Arc::new(AtomicBool::new(false)),
            fence_output: Arc::new(AtomicBool::new(false)),
            block_poll: Arc::new(AtomicBool::new(false)),
            block_step_heartbeat: Arc::new(AtomicBool::new(false)),
            complete_protocol_error: Arc::new(AtomicBool::new(false)),
            poll_observed: Arc::new(Notify::new()),
            heartbeat_observed: Arc::new(Notify::new()),
            step_heartbeat_count: Arc::new(AtomicUsize::new(0)),
            output_chunk_ids: Arc::new(Mutex::new(Vec::new())),
            failure: Arc::new(Mutex::new(None)),
            completion: Arc::new(Mutex::new(None)),
        }
    }
}

impl FakeApi {
    fn with_assignment(assignment: Assignment) -> Self {
        Self {
            assignment: Arc::new(Mutex::new(Some(assignment))),
            ..Self::default()
        }
    }

    fn record(&self, operation: &str) {
        self.calls
            .lock()
            .expect("calls lock")
            .push(operation.to_owned());
    }

    fn operations(&self) -> Vec<String> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn output_chunk_ids(&self) -> Vec<String> {
        self.output_chunk_ids
            .lock()
            .expect("output chunk IDs lock")
            .clone()
    }
}

impl WorkerApi for FakeApi {
    fn register_worker(&self, _registration: WorkerRegistration) -> WorkerFuture<'_, ()> {
        Box::pin(async move {
            self.record("register");
            Ok(())
        })
    }

    fn heartbeat_worker<'a>(
        &'a self,
        _worker_id: &'a str,
        drain: Option<bool>,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record(if drain == Some(true) {
                "worker-drain"
            } else {
                "worker-heartbeat"
            });
            Ok(())
        })
    }

    fn poll_worker<'a>(
        &'a self,
        _worker_id: &'a str,
        _wait_ms: u64,
    ) -> WorkerFuture<'a, WorkerPoll> {
        Box::pin(async move {
            self.record("poll");
            self.poll_observed.notify_one();
            if self.block_poll.load(Ordering::Acquire) {
                pending::<()>().await;
            }
            Ok(WorkerPoll {
                assignment: self.assignment.lock().expect("assignment lock").take(),
                retry_after_ms: 1,
            })
        })
    }

    fn start_step<'a>(&'a self, _step_id: &'a str, _lease: Lease) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("start");
            Ok(())
        })
    }

    fn heartbeat_step<'a>(&'a self, _step_id: &'a str, _lease: Lease) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("step-heartbeat");
            self.step_heartbeat_count.fetch_add(1, Ordering::AcqRel);
            self.heartbeat_observed.notify_one();
            if self.block_step_heartbeat.load(Ordering::Acquire) {
                pending::<()>().await;
            }
            if self.fence_heartbeat.load(Ordering::Acquire) {
                Err(DurableWorkerError::LeaseLost(
                    oresoftware_durable_worker::ProtocolError::new(
                        "lease_lost",
                        "fenced",
                        Some(409),
                        false,
                    ),
                ))
            } else {
                Ok(())
            }
        })
    }

    fn append_step_output<'a>(
        &'a self,
        _step_id: &'a str,
        output: StepOutput,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("output");
            self.output_chunk_ids
                .lock()
                .expect("output chunk IDs lock")
                .push(output.chunk_id);
            if self.fence_output.load(Ordering::Acquire) {
                Err(DurableWorkerError::LeaseLost(
                    oresoftware_durable_worker::ProtocolError::new(
                        "lease_lost",
                        "output fenced",
                        Some(409),
                        false,
                    ),
                ))
            } else {
                Ok(())
            }
        })
    }

    fn complete_step<'a>(
        &'a self,
        _step_id: &'a str,
        completion: StepCompletion,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("complete");
            if self.complete_protocol_error.load(Ordering::Acquire) {
                return Err(DurableWorkerError::Protocol(
                    oresoftware_durable_worker::ProtocolError::new(
                        "upstream_unavailable",
                        "completion result is unknown",
                        Some(503),
                        true,
                    ),
                ));
            }
            *self.completion.lock().expect("completion lock") = Some(completion);
            Ok(())
        })
    }

    fn fail_step<'a>(&'a self, _step_id: &'a str, failure: StepFailure) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("fail");
            *self.failure.lock().expect("failure lock") = Some(failure);
            Ok(())
        })
    }
}

fn assignment() -> Assignment {
    Assignment {
        run_id: "run-1".to_owned(),
        step_id: "step-1".to_owned(),
        step_key: "task".to_owned(),
        task_type: "demo".to_owned(),
        queue: "default".to_owned(),
        input: JsonObject::from_iter([("value".to_owned(), json!(7))]),
        attempt: 1,
        lease_token: "lease-token".to_owned(),
        lease_generation: 3,
        fencing_token: 9,
        lease_expires_at_ms: 4_102_444_800_000,
        timeout_ms: 60_000,
        affinity_key: None,
    }
}

fn config() -> WorkerConfig {
    WorkerConfig {
        worker_id: "rust-worker-1".to_owned(),
        queues: vec!["default".to_owned()],
        capabilities: vec!["demo".to_owned()],
        slots: 1,
        max_assignments: Some(1),
        poll_wait_ms: 1,
        worker_heartbeat_ms: 5,
        step_heartbeat_ms: 5,
        idle_sleep_ms: 1,
        ..WorkerConfig::default()
    }
}

#[test]
fn rejects_worker_heartbeat_cadence_that_can_expire_the_ttl() {
    let mut worker_config = config();
    worker_config.worker_heartbeat_ms = worker_config.ttl_ms;
    assert!(Worker::new(Arc::new(FakeApi::default()), HashMap::new(), worker_config,).is_err());
}

#[tokio::test]
async fn streams_progress_and_completes_under_the_same_generation() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    let handler: Handler = Arc::new(|context: TaskContext| {
        Box::pin(async move {
            context.emit("working", "progress", false).await?;
            tokio::time::sleep(Duration::from_millis(15)).await;
            context.emit("done", "progress", true).await?;
            let mut result = JsonObject::new();
            result.insert("answer".to_owned(), json!(14));
            Ok(result)
        })
    });
    let worker = Worker::new(
        api.clone(),
        HashMap::from([("demo".to_owned(), handler)]),
        config(),
    )
    .expect("worker");
    let summary = worker
        .run(Cancellation::default())
        .await
        .expect("worker run");
    assert_eq!(summary.accepted, 1);
    assert_eq!(summary.completed, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.protocol_errors, 0);
    assert!(api.step_heartbeat_count.load(Ordering::Acquire) > 0);
    let operations = api.operations();
    assert!(operations.contains(&"output".to_owned()));
    assert!(operations.contains(&"complete".to_owned()));
    assert_eq!(operations.last().map(String::as_str), Some("worker-drain"));
    assert_eq!(
        api.output_chunk_ids(),
        vec!["step-1:3:1".to_owned(), "step-1:3:2".to_owned()]
    );
    let completion = api
        .completion
        .lock()
        .expect("completion lock")
        .clone()
        .expect("completion");
    assert_eq!(completion.lease.lease_generation, 3);
    assert_eq!(completion.result.get("answer"), Some(&json!(14)));
}

#[tokio::test]
async fn fenced_heartbeat_aborts_non_cooperative_handler_and_suppresses_terminal_mutations() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    api.fence_heartbeat.store(true, Ordering::Release);
    let handler: Handler = Arc::new(|_context: TaskContext| {
        Box::pin(async move {
            pending::<()>().await;
            Ok(JsonObject::new())
        })
    });
    let worker = Worker::new(
        api.clone(),
        HashMap::from([("demo".to_owned(), handler)]),
        config(),
    )
    .expect("worker");
    let summary = tokio::time::timeout(
        Duration::from_millis(250),
        worker.run(Cancellation::default()),
    )
    .await
    .expect("lease loss should abort a non-cooperative handler promptly")
    .expect("worker run");
    assert_eq!(summary.lease_lost, 1);
    assert_eq!(summary.protocol_errors, 0);
    let operations = api.operations();
    assert!(!operations.contains(&"complete".to_owned()));
    assert!(!operations.contains(&"fail".to_owned()));
}

#[tokio::test]
async fn fenced_progress_output_cancels_handler_and_suppresses_terminal_mutations() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    api.fence_output.store(true, Ordering::Release);
    let handler: Handler = Arc::new(|context: TaskContext| {
        Box::pin(async move {
            context.emit("stale", "progress", false).await?;
            Ok(JsonObject::new())
        })
    });
    let worker = Worker::new(
        api.clone(),
        HashMap::from([("demo".to_owned(), handler)]),
        config(),
    )
    .expect("worker");
    let summary = worker
        .run(Cancellation::default())
        .await
        .expect("worker run");
    assert_eq!(summary.lease_lost, 1);
    assert_eq!(summary.protocol_errors, 0);
    assert_eq!(api.output_chunk_ids(), vec!["step-1:3:1".to_owned()]);
    let operations = api.operations();
    assert!(!operations.contains(&"complete".to_owned()));
    assert!(!operations.contains(&"fail".to_owned()));
}

#[tokio::test]
async fn handler_failure_preserves_explicit_retryability() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    let handler: Handler = Arc::new(|_context: TaskContext| {
        Box::pin(async move { Err(WorkerFailure::new("upstream_busy", "try later", true)) })
    });
    let worker = Worker::new(
        api.clone(),
        HashMap::from([("demo".to_owned(), handler)]),
        config(),
    )
    .expect("worker");
    let summary = worker
        .run(Cancellation::default())
        .await
        .expect("worker run");
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.protocol_errors, 0);
    let failure = api
        .failure
        .lock()
        .expect("failure lock")
        .clone()
        .expect("failure");
    assert_eq!(failure.code, "upstream_busy");
    assert!(failure.retryable);
}

#[tokio::test]
async fn missing_handler_is_terminal_and_non_retryable() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    let worker = Worker::new(api.clone(), HashMap::new(), config()).expect("worker");
    let summary = worker
        .run(Cancellation::default())
        .await
        .expect("worker run");
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.protocol_errors, 0);
    let failure = api
        .failure
        .lock()
        .expect("failure lock")
        .clone()
        .expect("failure");
    assert_eq!(failure.code, "handler_not_found");
    assert!(!failure.retryable);
}

#[tokio::test]
async fn assignment_timeout_aborts_handler_and_reports_retryable_failure() {
    let api = Arc::new(FakeApi::with_assignment({
        let mut assignment = assignment();
        assignment.timeout_ms = 20;
        assignment
    }));
    let handler: Handler = Arc::new(|_context: TaskContext| {
        Box::pin(async move {
            pending::<()>().await;
            Ok(JsonObject::new())
        })
    });
    let worker = Worker::new(
        api.clone(),
        HashMap::from([("demo".to_owned(), handler)]),
        config(),
    )
    .expect("worker");
    let summary = tokio::time::timeout(
        Duration::from_millis(250),
        worker.run(Cancellation::default()),
    )
    .await
    .expect("assignment timeout should abort the handler promptly")
    .expect("worker run");
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.lease_lost, 0);
    assert_eq!(summary.protocol_errors, 0);
    let failure = api
        .failure
        .lock()
        .expect("failure lock")
        .clone()
        .expect("failure");
    assert_eq!(failure.code, "handler_timeout");
    assert!(failure.retryable);
    assert!(!api.operations().contains(&"complete".to_owned()));
}

#[tokio::test]
async fn blocked_step_heartbeat_aborts_non_cooperative_handler_within_local_budget() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    api.block_step_heartbeat.store(true, Ordering::Release);
    let handler: Handler = Arc::new(|_context: TaskContext| {
        Box::pin(async move {
            pending::<()>().await;
            Ok(JsonObject::new())
        })
    });
    let worker = Worker::new(
        api.clone(),
        HashMap::from([("demo".to_owned(), handler)]),
        config(),
    )
    .expect("worker");
    let summary = tokio::time::timeout(
        Duration::from_millis(250),
        worker.run(Cancellation::default()),
    )
    .await
    .expect("heartbeat uncertainty should not occupy the slot indefinitely")
    .expect("worker run");
    assert_eq!(summary.lease_lost, 1);
    assert_eq!(summary.protocol_errors, 0);
    let operations = api.operations();
    assert!(!operations.contains(&"complete".to_owned()));
    assert!(!operations.contains(&"fail".to_owned()));
}

#[tokio::test]
async fn handler_panic_is_isolated_and_reported_as_a_terminal_failure() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    let handler: Handler = Arc::new(|_context: TaskContext| {
        Box::pin(async move {
            tokio::task::yield_now().await;
            panic!("boom")
        })
    });
    let worker = Worker::new(
        api.clone(),
        HashMap::from([("demo".to_owned(), handler)]),
        config(),
    )
    .expect("worker");
    let summary = worker
        .run(Cancellation::default())
        .await
        .expect("worker run");
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.protocol_errors, 0);
    let failure = api
        .failure
        .lock()
        .expect("failure lock")
        .clone()
        .expect("failure");
    assert_eq!(failure.code, "handler_panic");
    assert!(!failure.retryable);
}

#[tokio::test]
async fn ambiguous_terminal_mutation_is_counted_separately_from_acknowledged_failure() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    api.complete_protocol_error.store(true, Ordering::Release);
    let handler: Handler =
        Arc::new(|_context: TaskContext| Box::pin(async move { Ok(JsonObject::new()) }));
    let worker = Worker::new(
        api.clone(),
        HashMap::from([("demo".to_owned(), handler)]),
        config(),
    )
    .expect("worker");
    let summary = worker
        .run(Cancellation::default())
        .await
        .expect("worker run");
    assert_eq!(summary.completed, 0);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.protocol_errors, 1);
    assert!(api.failure.lock().expect("failure lock").is_none());
    assert!(api.completion.lock().expect("completion lock").is_none());
}

#[tokio::test]
async fn shutdown_cancels_an_in_flight_long_poll() {
    let api = Arc::new(FakeApi::default());
    api.block_poll.store(true, Ordering::Release);
    let mut worker_config = config();
    worker_config.max_assignments = None;
    worker_config.poll_wait_ms = 30_000;
    let worker = Worker::new(api.clone(), HashMap::new(), worker_config).expect("worker");
    let shutdown = Cancellation::default();
    let run_shutdown = shutdown.clone();
    let run = tokio::spawn(async move { worker.run(run_shutdown).await });
    api.poll_observed.notified().await;
    shutdown.cancel();
    let summary = tokio::time::timeout(Duration::from_millis(250), run)
        .await
        .expect("shutdown should interrupt the long poll")
        .expect("worker task")
        .expect("worker run");
    assert_eq!(summary.accepted, 0);
    assert_eq!(summary.protocol_errors, 0);
    assert_eq!(
        api.operations().last().map(String::as_str),
        Some("worker-drain")
    );
}
