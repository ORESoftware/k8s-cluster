use oresoftware_durable_worker::{
    Assignment, Cancellation, DurableWorkerError, Handler, JsonObject, Lease,
    StepCompletion, StepFailure, StepOutput, TaskContext, Worker, WorkerApi,
    WorkerConfig, WorkerFailure, WorkerFuture, WorkerPoll, WorkerRegistration,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct FakeApi {
    calls: Arc<Mutex<Vec<String>>>,
    assignment: Arc<Mutex<Option<Assignment>>>,
    fence_heartbeat: Arc<AtomicBool>,
    step_heartbeat_count: Arc<AtomicUsize>,
    failure: Arc<Mutex<Option<StepFailure>>>,
    completion: Arc<Mutex<Option<StepCompletion>>>,
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
}

impl WorkerApi for FakeApi {
    fn register_worker<'a>(
        &'a self,
        _registration: WorkerRegistration,
    ) -> WorkerFuture<'a, ()> {
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

    fn heartbeat_step<'a>(
        &'a self,
        _step_id: &'a str,
        _lease: Lease,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("step-heartbeat");
            self.step_heartbeat_count.fetch_add(1, Ordering::AcqRel);
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
        _output: StepOutput,
    ) -> WorkerFuture<'a, ()> {
        Box::pin(async move {
            self.record("output");
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
            *self.completion.lock().expect("completion lock") = Some(completion);
            Ok(())
        })
    }

    fn fail_step<'a>(
        &'a self,
        _step_id: &'a str,
        failure: StepFailure,
    ) -> WorkerFuture<'a, ()> {
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

#[tokio::test]
async fn streams_progress_and_completes_under_the_same_generation() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    let handler: Handler = Arc::new(|context: TaskContext| {
        Box::pin(async move {
            context.emit("working", "progress", false).await?;
            tokio::time::sleep(Duration::from_millis(15)).await;
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
    assert!(api.step_heartbeat_count.load(Ordering::Acquire) > 0);
    let operations = api.operations();
    assert!(operations.contains(&"output".to_owned()));
    assert!(operations.contains(&"complete".to_owned()));
    assert_eq!(operations.last().map(String::as_str), Some("worker-drain"));
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
async fn fenced_heartbeat_cancels_handler_and_suppresses_terminal_mutations() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    api.fence_heartbeat.store(true, Ordering::Release);
    let observed = Arc::new(AtomicBool::new(false));
    let handler_observed = Arc::clone(&observed);
    let handler: Handler = Arc::new(move |context: TaskContext| {
        let observed = Arc::clone(&handler_observed);
        Box::pin(async move {
            for _ in 0..200 {
                if context.cancellation().is_cancelled() {
                    observed.store(true, Ordering::Release);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            context.check_cancelled()?;
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
    assert!(observed.load(Ordering::Acquire));
    assert_eq!(summary.lease_lost, 1);
    let operations = api.operations();
    assert!(!operations.contains(&"complete".to_owned()));
    assert!(!operations.contains(&"fail".to_owned()));
}

#[tokio::test]
async fn handler_failure_preserves_explicit_retryability() {
    let api = Arc::new(FakeApi::with_assignment(assignment()));
    let handler: Handler = Arc::new(|_context: TaskContext| {
        Box::pin(async move {
            Err(WorkerFailure::new(
                "upstream_busy",
                "try later",
                true,
            ))
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
    let failure = api
        .failure
        .lock()
        .expect("failure lock")
        .clone()
        .expect("failure");
    assert_eq!(failure.code, "handler_not_found");
    assert!(!failure.retryable);
}
