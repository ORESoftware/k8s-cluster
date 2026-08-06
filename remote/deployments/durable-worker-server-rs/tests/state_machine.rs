use std::{collections::BTreeSet, sync::Arc};

use dd_durable_worker_server::{
    engine::{EngineError, ManualClock},
    model::{
        CompleteStepRequest, ConcurrencyPolicy, FailStepRequest, JsonObject, LeaseCommand,
        RetryPolicy, RunStatus, StepDefinition, StepOutputRequest, StepStatus, SubmitRunRequest,
        WorkerRegistration,
    },
    Engine, MemoryEventSink, MemoryStore,
};

fn worker(worker_id: &str, slots: u32) -> WorkerRegistration {
    WorkerRegistration {
        worker_id: worker_id.to_string(),
        queues: BTreeSet::from(["agents".to_string()]),
        capabilities: BTreeSet::from(["llm".to_string()]),
        labels: JsonObject::new(),
        slots,
        ttl_ms: 30_000,
        drain: None,
    }
}

fn step(key: &str, depends_on: &[&str]) -> StepDefinition {
    StepDefinition {
        key: key.to_string(),
        task_type: format!("agent:{key}"),
        queue: "agents".to_string(),
        input: JsonObject::new(),
        depends_on: depends_on
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        priority: 0,
        required_capabilities: BTreeSet::from(["llm".to_string()]),
        retry: RetryPolicy {
            max_attempts: 2,
            initial_backoff_ms: 1_000,
            max_backoff_ms: 4_000,
            multiplier: 2.0,
        },
        timeout_ms: 60_000,
        lease_ms: 5_000,
        not_before_ms: None,
        wait_for_signal: None,
        concurrency: None,
        affinity_key: None,
    }
}

fn engine(now_ms: u64) -> (Engine, Arc<ManualClock>, Arc<MemoryEventSink>) {
    let clock = Arc::new(ManualClock::new(now_ms));
    let events = Arc::new(MemoryEventSink::new());
    let engine = Engine::new(Arc::new(MemoryStore::new()), events.clone(), clock.clone());
    (engine, clock, events)
}

#[tokio::test]
async fn executes_a_dag_in_dependency_order() {
    let (engine, _, events) = engine(1_000_000);
    engine
        .register_worker(worker("node-worker-1", 2))
        .await
        .unwrap();
    let submitted = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: None,
            idempotency_key: Some("dag-order-1".to_string()),
            name: Some("research and summarize".to_string()),
            metadata: JsonObject::new(),
            steps: vec![step("research", &[]), step("summarize", &["research"])],
        })
        .await
        .unwrap();

    let first = engine
        .poll_once("node-worker-1")
        .await
        .unwrap()
        .assignment
        .unwrap();
    assert_eq!(first.step_key, "research");
    let first_lease = LeaseCommand {
        worker_id: "node-worker-1".to_string(),
        lease_token: first.lease_token.clone(),
        lease_generation: first.lease_generation,
    };
    engine
        .start_step(&first.step_id, first_lease)
        .await
        .unwrap();
    engine
        .complete_step(
            &first.step_id,
            CompleteStepRequest {
                worker_id: "node-worker-1".to_string(),
                lease_token: first.lease_token,
                lease_generation: first.lease_generation,
                result: JsonObject::new(),
            },
        )
        .await
        .unwrap();

    let second = engine
        .poll_once("node-worker-1")
        .await
        .unwrap()
        .assignment
        .unwrap();
    assert_eq!(second.step_key, "summarize");
    engine
        .complete_step(
            &second.step_id,
            CompleteStepRequest {
                worker_id: "node-worker-1".to_string(),
                lease_token: second.lease_token,
                lease_generation: second.lease_generation,
                result: JsonObject::new(),
            },
        )
        .await
        .unwrap();

    let snapshot = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(snapshot.run.status, RunStatus::Succeeded);
    assert!(snapshot
        .steps
        .iter()
        .all(|step| step.status == StepStatus::Succeeded));
    let event_types = events
        .snapshot()
        .await
        .into_iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"run.submitted".to_string()));
    assert!(event_types.contains(&"run.succeeded".to_string()));
}

#[tokio::test]
async fn lease_expiration_retries_then_fences_the_stale_worker() {
    let (engine, clock, _) = engine(2_000_000);
    engine
        .register_worker(worker("rust-worker-1", 1))
        .await
        .unwrap();
    let submitted = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: None,
            idempotency_key: None,
            name: None,
            metadata: JsonObject::new(),
            steps: vec![step("slow", &[])],
        })
        .await
        .unwrap();
    let first = engine
        .poll_once("rust-worker-1")
        .await
        .unwrap()
        .assignment
        .unwrap();

    clock.advance(5_001);
    engine.tick().await.unwrap();
    let waiting = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(waiting.steps[0].status, StepStatus::WaitingRetry);

    clock.advance(1_001);
    engine.tick().await.unwrap();
    let second = engine
        .poll_once("rust-worker-1")
        .await
        .unwrap()
        .assignment
        .unwrap();
    assert_eq!(second.attempt, 2);
    assert!(second.lease_generation > first.lease_generation);

    let stale = engine
        .complete_step(
            &first.step_id,
            CompleteStepRequest {
                worker_id: "rust-worker-1".to_string(),
                lease_token: first.lease_token,
                lease_generation: first.lease_generation,
                result: JsonObject::new(),
            },
        )
        .await;
    assert!(matches!(stale, Err(EngineError::Conflict(_))));

    engine
        .fail_step(
            &second.step_id,
            FailStepRequest {
                worker_id: "rust-worker-1".to_string(),
                lease_token: second.lease_token,
                lease_generation: second.lease_generation,
                code: "model_error".to_string(),
                message: "provider failed".to_string(),
                retryable: true,
            },
        )
        .await
        .unwrap();
    let failed = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(failed.run.status, RunStatus::Failed);
    assert_eq!(failed.steps[0].status, StepStatus::Failed);
}

#[tokio::test]
async fn keyed_concurrency_is_enforced_across_runs() {
    let (engine, _, _) = engine(3_000_000);
    engine
        .register_worker(worker("gleam-worker-1", 2))
        .await
        .unwrap();
    let mut first_step = step("one", &[]);
    first_step.concurrency = Some(ConcurrencyPolicy {
        key: "tenant:acme:llm".to_string(),
        limit: 1,
    });
    let mut second_step = step("two", &[]);
    second_step.concurrency = first_step.concurrency.clone();
    engine
        .submit_run(SubmitRunRequest {
            deadline_ms: None,
            idempotency_key: None,
            name: None,
            metadata: JsonObject::new(),
            steps: vec![first_step],
        })
        .await
        .unwrap();
    engine
        .submit_run(SubmitRunRequest {
            deadline_ms: None,
            idempotency_key: None,
            name: None,
            metadata: JsonObject::new(),
            steps: vec![second_step],
        })
        .await
        .unwrap();

    let first = engine
        .poll_once("gleam-worker-1")
        .await
        .unwrap()
        .assignment
        .unwrap();
    assert!(engine
        .poll_once("gleam-worker-1")
        .await
        .unwrap()
        .assignment
        .is_none());
    engine
        .complete_step(
            &first.step_id,
            CompleteStepRequest {
                worker_id: "gleam-worker-1".to_string(),
                lease_token: first.lease_token,
                lease_generation: first.lease_generation,
                result: JsonObject::new(),
            },
        )
        .await
        .unwrap();
    assert!(engine
        .poll_once("gleam-worker-1")
        .await
        .unwrap()
        .assignment
        .is_some());
}

#[tokio::test]
async fn idempotency_replays_the_original_run_and_rejects_payload_drift() {
    let (engine, _, _) = engine(4_000_000);
    let request = SubmitRunRequest {
        deadline_ms: None,
        idempotency_key: Some("same-request".to_string()),
        name: None,
        metadata: JsonObject::new(),
        steps: vec![step("task", &[])],
    };
    let first = engine.submit_run(request.clone()).await.unwrap();
    let replay = engine.submit_run(request.clone()).await.unwrap();
    assert_eq!(first.run_id, replay.run_id);
    assert!(replay.idempotent_replay);

    let mut changed = request;
    changed.steps[0].priority = 99;
    assert!(matches!(
        engine.submit_run(changed).await,
        Err(EngineError::IdempotencyMismatch)
    ));
}

#[tokio::test]
async fn heartbeat_extends_worker_and_keyed_concurrency_lanes() {
    let (engine, clock, _) = engine(5_000_000);
    engine
        .register_worker(worker("node-worker-lanes", 2))
        .await
        .unwrap();

    for key in ["first", "second"] {
        let mut definition = step(key, &[]);
        definition.concurrency = Some(ConcurrencyPolicy {
            key: "tenant:acme:provider:openai".to_string(),
            limit: 1,
        });
        engine
            .submit_run(SubmitRunRequest {
                deadline_ms: None,
                idempotency_key: None,
                name: None,
                metadata: JsonObject::new(),
                steps: vec![definition],
            })
            .await
            .unwrap();
    }

    let first = engine
        .poll_once("node-worker-lanes")
        .await
        .unwrap()
        .assignment
        .unwrap();
    let command = LeaseCommand {
        worker_id: "node-worker-lanes".to_string(),
        lease_token: first.lease_token.clone(),
        lease_generation: first.lease_generation,
    };
    engine
        .start_step(&first.step_id, command.clone())
        .await
        .unwrap();

    clock.advance(4_000);
    engine
        .heartbeat_step(&first.step_id, command.clone())
        .await
        .unwrap();
    clock.advance(1_500);
    engine.tick().await.unwrap();

    assert!(engine
        .poll_once("node-worker-lanes")
        .await
        .unwrap()
        .assignment
        .is_none());

    engine
        .complete_step(
            &first.step_id,
            CompleteStepRequest {
                worker_id: command.worker_id,
                lease_token: command.lease_token,
                lease_generation: command.lease_generation,
                result: JsonObject::new(),
            },
        )
        .await
        .unwrap();
    assert!(engine
        .poll_once("node-worker-lanes")
        .await
        .unwrap()
        .assignment
        .is_some());
}

#[tokio::test]
async fn hard_timeout_wins_even_when_heartbeats_keep_the_lease_alive() {
    let (engine, clock, _) = engine(6_000_000);
    engine
        .register_worker(worker("rust-worker-timeout", 1))
        .await
        .unwrap();
    let mut definition = step("bounded", &[]);
    definition.timeout_ms = 6_000;
    definition.lease_ms = 5_000;
    let submitted = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: None,
            idempotency_key: None,
            name: None,
            metadata: JsonObject::new(),
            steps: vec![definition],
        })
        .await
        .unwrap();
    let assignment = engine
        .poll_once("rust-worker-timeout")
        .await
        .unwrap()
        .assignment
        .unwrap();
    let command = LeaseCommand {
        worker_id: "rust-worker-timeout".to_string(),
        lease_token: assignment.lease_token,
        lease_generation: assignment.lease_generation,
    };
    engine
        .start_step(&assignment.step_id, command.clone())
        .await
        .unwrap();

    clock.advance(4_000);
    engine
        .heartbeat_step(&assignment.step_id, command)
        .await
        .unwrap();
    clock.advance(2_500);
    engine.tick().await.unwrap();

    let snapshot = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(snapshot.steps[0].status, StepStatus::WaitingRetry);
    assert_eq!(
        snapshot.steps[0].failure.as_ref().unwrap().code,
        "step_timeout"
    );
    assert_eq!(
        engine
            .metrics()
            .step_timeouts_total
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn terminal_failure_cancels_descendants_and_independent_siblings() {
    let (engine, _, events) = engine(7_000_000);
    engine
        .register_worker(worker("gleam-worker-fail-fast", 2))
        .await
        .unwrap();
    let mut root = step("root", &[]);
    root.priority = 100;
    let submitted = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: None,
            idempotency_key: None,
            name: None,
            metadata: JsonObject::new(),
            steps: vec![
                root,
                step("child", &["root"]),
                step("grandchild", &["child"]),
                step("independent", &[]),
            ],
        })
        .await
        .unwrap();
    let assignment = engine
        .poll_once("gleam-worker-fail-fast")
        .await
        .unwrap()
        .assignment
        .unwrap();
    assert_eq!(assignment.step_key, "root");

    engine
        .fail_step(
            &assignment.step_id,
            FailStepRequest {
                worker_id: "gleam-worker-fail-fast".to_string(),
                lease_token: assignment.lease_token,
                lease_generation: assignment.lease_generation,
                code: "terminal_model_failure".to_string(),
                message: "do not continue this graph".to_string(),
                retryable: false,
            },
        )
        .await
        .unwrap();

    let snapshot = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(snapshot.run.status, RunStatus::Failed);
    let statuses = snapshot
        .steps
        .iter()
        .map(|step| (step.key.as_str(), step.status))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(statuses["root"], StepStatus::Failed);
    assert_eq!(statuses["child"], StepStatus::Cancelled);
    assert_eq!(statuses["grandchild"], StepStatus::Cancelled);
    assert_eq!(statuses["independent"], StepStatus::Cancelled);
    assert_eq!(snapshot.run.counts.failed, 1);
    assert_eq!(snapshot.run.counts.cancelled, 3);

    let cancelled_events = events
        .snapshot()
        .await
        .into_iter()
        .filter(|event| event.event_type == "step.cancelled")
        .count();
    assert_eq!(cancelled_events, 3);
}

#[tokio::test]
async fn terminal_commands_and_output_chunks_are_idempotent() {
    let (engine, _, events) = engine(8_000_000);
    engine
        .register_worker(worker("node-worker-idempotent", 1))
        .await
        .unwrap();
    engine
        .submit_run(SubmitRunRequest {
            deadline_ms: None,
            idempotency_key: None,
            name: None,
            metadata: JsonObject::new(),
            steps: vec![step("stream", &[])],
        })
        .await
        .unwrap();
    let assignment = engine
        .poll_once("node-worker-idempotent")
        .await
        .unwrap()
        .assignment
        .unwrap();
    let command = LeaseCommand {
        worker_id: "node-worker-idempotent".to_string(),
        lease_token: assignment.lease_token.clone(),
        lease_generation: assignment.lease_generation,
    };
    engine
        .start_step(&assignment.step_id, command.clone())
        .await
        .unwrap();

    let output = StepOutputRequest {
        chunk_id: "chunk-0001".to_string(),
        worker_id: command.worker_id.clone(),
        lease_token: command.lease_token.clone(),
        lease_generation: command.lease_generation,
        stream: Some("assistant".to_string()),
        chunk: "partial result".to_string(),
        final_chunk: Some(false),
    };
    engine
        .append_output(&assignment.step_id, output.clone())
        .await
        .unwrap();
    engine
        .append_output(&assignment.step_id, output.clone())
        .await
        .unwrap();
    assert_eq!(
        engine
            .get_step(&assignment.step_id)
            .await
            .unwrap()
            .output_sequence,
        1
    );
    assert_eq!(
        events
            .snapshot()
            .await
            .into_iter()
            .filter(|event| event.event_type == "step.output")
            .count(),
        1
    );

    let mut changed_output = output;
    changed_output.chunk = "different data".to_string();
    assert!(matches!(
        engine
            .append_output(&assignment.step_id, changed_output)
            .await,
        Err(EngineError::InvalidRequest(_))
    ));

    let completion = CompleteStepRequest {
        worker_id: command.worker_id,
        lease_token: command.lease_token,
        lease_generation: command.lease_generation,
        result: JsonObject::new(),
    };
    engine
        .complete_step(&assignment.step_id, completion.clone())
        .await
        .unwrap();
    engine
        .complete_step(&assignment.step_id, completion)
        .await
        .unwrap();
}

#[tokio::test]
async fn repeated_failure_acknowledgement_returns_the_committed_retry_state() {
    let (engine, _, _) = engine(9_000_000);
    engine
        .register_worker(worker("rust-worker-failure-idempotency", 1))
        .await
        .unwrap();
    engine
        .submit_run(SubmitRunRequest {
            deadline_ms: None,
            idempotency_key: None,
            name: None,
            metadata: JsonObject::new(),
            steps: vec![step("retry", &[])],
        })
        .await
        .unwrap();
    let assignment = engine
        .poll_once("rust-worker-failure-idempotency")
        .await
        .unwrap()
        .assignment
        .unwrap();
    let failure = FailStepRequest {
        worker_id: "rust-worker-failure-idempotency".to_string(),
        lease_token: assignment.lease_token,
        lease_generation: assignment.lease_generation,
        code: "temporary".to_string(),
        message: "retry me".to_string(),
        retryable: true,
    };
    let first = engine
        .fail_step(&assignment.step_id, failure.clone())
        .await
        .unwrap();
    let replay = engine
        .fail_step(&assignment.step_id, failure)
        .await
        .unwrap();
    assert_eq!(first.status.as_deref(), Some("waiting_retry"));
    assert_eq!(replay.status.as_deref(), Some("waiting_retry"));
}

#[tokio::test]
async fn rejects_expired_deadline_without_poisoning_idempotency() {
    let (engine, _, _) = engine(8_000_000);
    let error = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: Some(8_000_000),
            idempotency_key: Some("deadline-not-poisoned".to_string()),
            name: Some("expired".to_string()),
            metadata: JsonObject::new(),
            steps: vec![step("expired", &[])],
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::InvalidRequest(message) if message.contains("deadlineMs")
    ));

    let accepted = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: Some(8_001_000),
            idempotency_key: Some("deadline-not-poisoned".to_string()),
            name: Some("valid retry".to_string()),
            metadata: JsonObject::new(),
            steps: vec![step("expired", &[])],
        })
        .await
        .unwrap();
    assert_eq!(accepted.status, RunStatus::Pending);
}

#[tokio::test]
async fn deadline_is_part_of_the_idempotency_binding() {
    let (engine, _, _) = engine(9_000_000);
    let request = SubmitRunRequest {
        deadline_ms: Some(9_100_000),
        idempotency_key: Some("deadline-idempotency".to_string()),
        name: Some("deadline binding".to_string()),
        metadata: JsonObject::new(),
        steps: vec![step("bound", &[])],
    };
    engine.submit_run(request.clone()).await.unwrap();
    let mut changed = request;
    changed.deadline_ms = Some(9_200_000);
    let error = engine.submit_run(changed).await.unwrap_err();
    assert!(matches!(error, EngineError::IdempotencyMismatch));
}

#[tokio::test]
async fn idempotent_retry_after_deadline_returns_the_failed_run() {
    let (engine, clock, _) = engine(9_500_000);
    let request = SubmitRunRequest {
        deadline_ms: Some(9_500_100),
        idempotency_key: Some("deadline-replay".to_string()),
        name: Some("deadline replay".to_string()),
        metadata: JsonObject::new(),
        steps: vec![step("queued", &[])],
    };
    let submitted = engine.submit_run(request.clone()).await.unwrap();
    clock.advance(101);
    let replay = engine.submit_run(request).await.unwrap();
    assert_eq!(replay.run_id, submitted.run_id);
    assert!(replay.idempotent_replay);
    assert_eq!(replay.status, RunStatus::Failed);
    let snapshot = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(snapshot.run.status, RunStatus::Failed);
    assert_eq!(snapshot.steps[0].status, StepStatus::Cancelled);
}

#[tokio::test]
async fn run_deadline_fences_active_work_and_remains_terminal() {
    let (engine, clock, events) = engine(10_000_000);
    engine
        .register_worker(worker("deadline-worker", 1))
        .await
        .unwrap();
    let submitted = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: Some(10_000_500),
            idempotency_key: Some("deadline-fencing".to_string()),
            name: Some("deadline fencing".to_string()),
            metadata: JsonObject::new(),
            steps: vec![step("active", &[]), step("queued", &[])],
        })
        .await
        .unwrap();

    let assignment = engine
        .poll_once("deadline-worker")
        .await
        .unwrap()
        .assignment
        .unwrap();
    engine
        .start_step(
            &assignment.step_id,
            LeaseCommand {
                worker_id: "deadline-worker".to_string(),
                lease_token: assignment.lease_token.clone(),
                lease_generation: assignment.lease_generation,
            },
        )
        .await
        .unwrap();

    clock.advance(501);
    engine.tick().await.unwrap();
    let expired = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(expired.run.status, RunStatus::Failed);
    assert_eq!(expired.run.deadline_ms, Some(10_000_500));
    assert_eq!(expired.run.counts.cancelled, 2);
    assert!(expired
        .steps
        .iter()
        .all(|step| step.status == StepStatus::Cancelled));

    let stale = engine
        .complete_step(
            &assignment.step_id,
            CompleteStepRequest {
                worker_id: "deadline-worker".to_string(),
                lease_token: assignment.lease_token,
                lease_generation: assignment.lease_generation,
                result: JsonObject::new(),
            },
        )
        .await;
    assert!(matches!(stale, Err(EngineError::Conflict(_))));

    engine.tick().await.unwrap();
    let still_failed = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(still_failed.run.status, RunStatus::Failed);
    assert!(engine
        .metrics()
        .render_prometheus()
        .contains("dd_durable_run_deadlines_exceeded_total 1"));
    let event_types = events
        .snapshot()
        .await
        .into_iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"run.deadline_exceeded".to_string()));
    assert!(event_types.contains(&"step.cancelled".to_string()));
}
