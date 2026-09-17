use oresoftware_durable_worker::{
    Assignment, Cancellation, DurableWorkerError, Handler, JsonObject, Lease, ProtocolError,
    StepCompletion, StepFailure, StepOutput, TaskContext, Worker, WorkerApi, WorkerConfig,
    WorkerFailure, WorkerFuture, WorkerPoll, WorkerRegistration, WorkerSummary,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct AdversarialApi {
    calls: Mutex<Vec<String>>,
    assignment: Mutex<Option<Assignment>>,
    outputs: Mutex<Vec<StepOutput>>,
    completion: Mutex<Option<StepCompletion>>,
    failure: Mutex<Option<StepFailure>>,
    registration_count: AtomicUsize,
    poll_count: AtomicUsize,
    drain_count: AtomicUsize,
    start_lease_lost: AtomicBool,
    start_protocol_error: AtomicBool,
    complete_lease_lost: AtomicBool,
    complete_protocol_error: AtomicBool,
    fail_lease_lost: AtomicBool,
    fail_protocol_error: AtomicBool,
}

impl AdversarialApi {
    fn with_assignment(assignment: Assignment) -> Self {
        Self {
            assignment: Mutex::new(Some(assignment)),
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

    fn lease_lost(message: &str) -> DurableWorkerError {
        DurableWorkerError::LeaseLost(ProtocolError::new("lease_lost", message, Some(409), false))
    }

    fn protocol_error(message: &str) -> DurableWorkerError {
        DurableWorkerError::Protocol(ProtocolError::new(
            "upstream_unavailable",
            message,
            Some(503),
            true,
        ))
    }
}

impl WorkerApi for AdversarialApi {
    fn register_worker(&self, _registration: WorkerRegistration) -> WorkerFuture<'_, ()> {
        Box::pin(async move {
            self.registration_count.fetch_add(1, Ordering::AcqRel);
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
            if drain == Some(true) {
                self.drain_count.fetch_add(1, Ordering::AcqRel);
                self.record("worker-drain");
            } else {
                self.record("worker-heartbeat");
            }
            Ok(())
        })
    }

    fn poll_worker<'a>(
        &'a self,
        _worker_id: &'a str,
        _wait_ms: u64,
    ) -> WorkerFuture<'a, WorkerPoll> {
        Box::pin(async move {
            self.poll_count.fetch_add(1, Ordering::AcqRel);
            self.record("poll");
            Ok(WorkerPoll {
                assignment: self.assignment.lock().expect("assignment lock").take(),
                retry_after_ms: 1,
            })
        })
    }

    fn start_step<'a>(&'a self, _step_id: &'a str, _lease: Lease) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("start");
            if self.start_lease_lost.load(Ordering::Acquire) {
                Err(Self::lease_lost("start fenced"))
            } else if self.start_protocol_error.load(Ordering::Acquire) {
                Err(Self::protocol_error("start result unknown"))
            } else {
                Ok(())
            }
        })
    }

    fn heartbeat_step<'a>(&'a self, _step_id: &'a str, _lease: Lease) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("step-heartbeat");
            Ok(())
        })
    }

    fn append_step_output<'a>(
        &'a self,
        _step_id: &'a str,
        output: StepOutput,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("output");
            self.outputs.lock().expect("outputs lock").push(output);
            Ok(())
        })
    }

    fn complete_step<'a>(
        &'a self,
        _step_id: &'a str,
        completion: StepCompletion,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("complete");
            if self.complete_lease_lost.load(Ordering::Acquire) {
                Err(Self::lease_lost("completion fenced"))
            } else if self.complete_protocol_error.load(Ordering::Acquire) {
                Err(Self::protocol_error("completion result unknown"))
            } else {
                *self.completion.lock().expect("completion lock") = Some(completion);
                Ok(())
            }
        })
    }

    fn fail_step<'a>(&'a self, _step_id: &'a str, failure: StepFailure) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("fail");
            if self.fail_lease_lost.load(Ordering::Acquire) {
                Err(Self::lease_lost("failure fenced"))
            } else if self.fail_protocol_error.load(Ordering::Acquire) {
                Err(Self::protocol_error("failure result unknown"))
            } else {
                *self.failure.lock().expect("failure lock") = Some(failure);
                Ok(())
            }
        })
    }
}

fn assignment() -> Assignment {
    Assignment {
        run_id: "run-adversarial".to_owned(),
        step_id: "step-adversarial".to_owned(),
        step_key: "task".to_owned(),
        task_type: "demo".to_owned(),
        queue: "default".to_owned(),
        input: JsonObject::from_iter([("value".to_owned(), json!(7))]),
        attempt: 1,
        lease_token: "lease-token-adversarial".to_owned(),
        lease_generation: 11,
        fencing_token: 17,
        lease_expires_at_ms: 4_102_444_800_000,
        timeout_ms: 10_000,
        affinity_key: None,
    }
}

fn config() -> WorkerConfig {
    WorkerConfig {
        worker_id: "rust-worker-adversarial".to_owned(),
        queues: vec!["default".to_owned()],
        capabilities: vec!["demo".to_owned()],
        slots: 1,
        ttl_ms: 5_000,
        poll_wait_ms: 1,
        worker_heartbeat_ms: 1_000,
        step_heartbeat_ms: 1_000,
        max_assignments: Some(1),
        idle_sleep_ms: 1,
        ..WorkerConfig::default()
    }
}

fn ok_handler() -> Handler {
    Arc::new(|_context: TaskContext| {
        Box::pin(async move {
            let mut result = JsonObject::new();
            result.insert("ok".to_owned(), json!(true));
            Ok(result)
        })
    })
}

fn failure_handler(retryable: bool) -> Handler {
    Arc::new(move |_context: TaskContext| {
        Box::pin(async move {
            Err(WorkerFailure::new(
                "handler_failed",
                "handler returned an expected failure",
                retryable,
            ))
        })
    })
}

async fn run_one(api: Arc<AdversarialApi>, handler: Option<Handler>) -> WorkerSummary {
    let handlers = handler
        .map(|handler| HashMap::from([("demo".to_owned(), handler)]))
        .unwrap_or_default();
    Worker::new(api, handlers, config())
        .expect("worker")
        .run(Cancellation::default())
        .await
        .expect("worker run")
}

fn assert_invalid_assignment(summary: &WorkerSummary, api: &AdversarialApi) {
    assert_eq!(summary.accepted, 1);
    assert_eq!(summary.completed, 0);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.lease_lost, 0);
    assert_eq!(summary.protocol_errors, 1);
    let operations = api.operations();
    assert!(!operations.iter().any(|operation| operation == "start"));
    assert!(!operations.iter().any(|operation| operation == "complete"));
    assert!(!operations.iter().any(|operation| operation == "fail"));
    assert_eq!(api.drain_count.load(Ordering::Acquire), 1);
}

// 1. Invalid assignments with an empty step identity fail before durable start.
#[tokio::test]
async fn rejects_assignment_with_empty_step_id_before_start() {
    let mut value = assignment();
    value.step_id.clear();
    let api = Arc::new(AdversarialApi::with_assignment(value));
    let summary = run_one(api.clone(), Some(ok_handler())).await;
    assert_invalid_assignment(&summary, &api);
}

// 2. Invalid assignments with an empty task type fail before handler lookup/start.
#[tokio::test]
async fn rejects_assignment_with_empty_task_type_before_start() {
    let mut value = assignment();
    value.task_type.clear();
    let api = Arc::new(AdversarialApi::with_assignment(value));
    let summary = run_one(api.clone(), Some(ok_handler())).await;
    assert_invalid_assignment(&summary, &api);
}

// 3. A missing lease token never reaches a terminal mutation.
#[tokio::test]
async fn rejects_assignment_with_empty_lease_token_before_start() {
    let mut value = assignment();
    value.lease_token.clear();
    let api = Arc::new(AdversarialApi::with_assignment(value));
    let summary = run_one(api.clone(), Some(ok_handler())).await;
    assert_invalid_assignment(&summary, &api);
}

// 4. Non-positive generations are not admitted as authoritative leases.
#[tokio::test]
async fn rejects_assignment_with_non_positive_lease_generation_before_start() {
    let mut value = assignment();
    value.lease_generation = -1;
    let api = Arc::new(AdversarialApi::with_assignment(value));
    let summary = run_one(api.clone(), Some(ok_handler())).await;
    assert_invalid_assignment(&summary, &api);
}

// 5. A zero handler budget is a protocol error, not an unbounded execution request.
#[tokio::test]
async fn rejects_assignment_with_zero_timeout_before_start() {
    let mut value = assignment();
    value.timeout_ms = 0;
    let api = Arc::new(AdversarialApi::with_assignment(value));
    let summary = run_one(api.clone(), Some(ok_handler())).await;
    assert_invalid_assignment(&summary, &api);
}

// 6. Losing the lease during start is classified as lease loss and suppresses terminals.
#[tokio::test]
async fn start_step_lease_loss_suppresses_handler_and_terminal_mutations() {
    let api = Arc::new(AdversarialApi::with_assignment(assignment()));
    api.start_lease_lost.store(true, Ordering::Release);
    let summary = run_one(api.clone(), Some(ok_handler())).await;
    assert_eq!(summary.lease_lost, 1);
    assert_eq!(summary.protocol_errors, 0);
    let operations = api.operations();
    assert!(operations.iter().any(|operation| operation == "start"));
    assert!(!operations.iter().any(|operation| operation == "complete"));
    assert!(!operations.iter().any(|operation| operation == "fail"));
}

// 7. An ambiguous/non-lease start failure stays a protocol error and never dispatches the handler.
#[tokio::test]
async fn start_step_protocol_error_is_not_misreported_as_handler_failure() {
    let api = Arc::new(AdversarialApi::with_assignment(assignment()));
    api.start_protocol_error.store(true, Ordering::Release);
    let summary = run_one(api.clone(), Some(ok_handler())).await;
    assert_eq!(summary.protocol_errors, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.lease_lost, 0);
    let operations = api.operations();
    assert!(!operations.iter().any(|operation| operation == "complete"));
    assert!(!operations.iter().any(|operation| operation == "fail"));
}

// 8. A completion fence is lease loss, not a successful or retryable handler failure.
#[tokio::test]
async fn completion_lease_loss_is_not_counted_as_completed_or_failed() {
    let api = Arc::new(AdversarialApi::with_assignment(assignment()));
    api.complete_lease_lost.store(true, Ordering::Release);
    let summary = run_one(api.clone(), Some(ok_handler())).await;
    assert_eq!(summary.completed, 0);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.lease_lost, 1);
    assert_eq!(summary.protocol_errors, 0);
    assert!(api.failure.lock().expect("failure lock").is_none());
}

// 9. A failure-report fence is lease loss and must not be acknowledged as failed.
#[tokio::test]
async fn failure_lease_loss_is_not_counted_as_acknowledged_failure() {
    let api = Arc::new(AdversarialApi::with_assignment(assignment()));
    api.fail_lease_lost.store(true, Ordering::Release);
    let summary = run_one(api.clone(), Some(failure_handler(true))).await;
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.lease_lost, 1);
    assert_eq!(summary.protocol_errors, 0);
    assert!(api.failure.lock().expect("failure lock").is_none());
}

// 10. Ambiguous failure persistence is kept separate from acknowledged handler failure.
#[tokio::test]
async fn failure_protocol_error_is_counted_as_protocol_ambiguity() {
    let api = Arc::new(AdversarialApi::with_assignment(assignment()));
    api.fail_protocol_error.store(true, Ordering::Release);
    let summary = run_one(api.clone(), Some(failure_handler(true))).await;
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.lease_lost, 0);
    assert_eq!(summary.protocol_errors, 1);
    assert!(api.failure.lock().expect("failure lock").is_none());
}

// 11. A pre-cancelled worker still registers/drains once but never polls for new work.
#[tokio::test]
async fn pre_cancelled_shutdown_never_polls_and_emits_one_drain() {
    let api = Arc::new(AdversarialApi::default());
    let mut worker_config = config();
    worker_config.max_assignments = None;
    let worker = Worker::new(api.clone(), HashMap::new(), worker_config).expect("worker");
    let shutdown = Cancellation::default();
    shutdown.cancel();
    let summary = worker.run(shutdown).await.expect("worker run");
    assert_eq!(summary, WorkerSummary::default());
    assert_eq!(api.registration_count.load(Ordering::Acquire), 1);
    assert_eq!(api.poll_count.load(Ordering::Acquire), 0);
    assert_eq!(api.drain_count.load(Ordering::Acquire), 1);
    assert_eq!(api.operations(), vec!["register", "worker-drain"]);
}

// 12. A zero assignment budget is a valid no-work drain and never starts a poll.
#[tokio::test]
async fn zero_max_assignments_registers_and_drains_without_polling() {
    let api = Arc::new(AdversarialApi::default());
    let mut worker_config = config();
    worker_config.max_assignments = Some(0);
    let worker = Worker::new(api.clone(), HashMap::new(), worker_config).expect("worker");
    let summary = worker
        .run(Cancellation::default())
        .await
        .expect("worker run");
    assert_eq!(summary, WorkerSummary::default());
    assert_eq!(api.registration_count.load(Ordering::Acquire), 1);
    assert_eq!(api.poll_count.load(Ordering::Acquire), 0);
    assert_eq!(api.drain_count.load(Ordering::Acquire), 1);
}

// 13. Cancellation is idempotent and both synchronous/asynchronous observers see it immediately.
#[tokio::test]
async fn cancellation_is_idempotent_and_pre_cancelled_waiters_complete_immediately() {
    let cancellation = Cancellation::default();
    cancellation.cancel();
    cancellation.cancel();
    assert!(cancellation.is_cancelled());
    let error = cancellation
        .check()
        .expect_err("cancelled lease must fail check");
    assert!(error.is_lease_lost());
    tokio::time::timeout(Duration::from_millis(25), cancellation.cancelled())
        .await
        .expect("pre-cancelled waiter must return immediately");
}

// 14. Progress IDs, finality, and durable terminal lease all stay on one generation.
#[tokio::test]
async fn progress_sequence_and_completion_remain_bound_to_one_lease_generation() {
    let api = Arc::new(AdversarialApi::with_assignment(assignment()));
    let handler: Handler = Arc::new(|context: TaskContext| {
        Box::pin(async move {
            context.emit("one", "progress", false).await?;
            context.emit("two", "progress", false).await?;
            context.emit("three", "progress", true).await?;
            Ok(JsonObject::from_iter([("done".to_owned(), json!(true))]))
        })
    });
    let summary = run_one(api.clone(), Some(handler)).await;
    assert_eq!(summary.completed, 1);
    let outputs = api.outputs.lock().expect("outputs lock");
    assert_eq!(outputs.len(), 3);
    assert_eq!(outputs[0].chunk_id, "step-adversarial:11:1");
    assert_eq!(outputs[1].chunk_id, "step-adversarial:11:2");
    assert_eq!(outputs[2].chunk_id, "step-adversarial:11:3");
    assert!(!outputs[0].final_chunk);
    assert!(!outputs[1].final_chunk);
    assert!(outputs[2].final_chunk);
    for output in outputs.iter() {
        assert_eq!(output.lease.worker_id, "rust-worker-adversarial");
        assert_eq!(output.lease.lease_token, "lease-token-adversarial");
        assert_eq!(output.lease.lease_generation, 11);
    }
    drop(outputs);
    let completion = api.completion.lock().expect("completion lock");
    let completion = completion.as_ref().expect("completion");
    assert_eq!(completion.lease.worker_id, "rust-worker-adversarial");
    assert_eq!(completion.lease.lease_token, "lease-token-adversarial");
    assert_eq!(completion.lease.lease_generation, 11);
    assert_eq!(api.drain_count.load(Ordering::Acquire), 1);
}

// 15. Acknowledged failures retain exact lease identity and failure semantics through drain.
#[tokio::test]
async fn acknowledged_failure_preserves_lease_identity_and_terminal_semantics() {
    let api = Arc::new(AdversarialApi::with_assignment(assignment()));
    let summary = run_one(api.clone(), Some(failure_handler(false))).await;
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.protocol_errors, 0);
    let failure = api.failure.lock().expect("failure lock");
    let failure = failure.as_ref().expect("failure");
    assert_eq!(failure.lease.worker_id, "rust-worker-adversarial");
    assert_eq!(failure.lease.lease_token, "lease-token-adversarial");
    assert_eq!(failure.lease.lease_generation, 11);
    assert_eq!(failure.code, "handler_failed");
    assert_eq!(failure.message, "handler returned an expected failure");
    assert!(!failure.retryable);
    drop(failure);
    assert_eq!(api.drain_count.load(Ordering::Acquire), 1);
    assert_eq!(
        api.operations().last().map(String::as_str),
        Some("worker-drain")
    );
}
