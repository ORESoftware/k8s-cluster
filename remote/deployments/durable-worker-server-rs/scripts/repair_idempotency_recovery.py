#!/usr/bin/env python3
from pathlib import Path
from textwrap import dedent, indent

engine_path = Path("remote/deployments/durable-worker-server-rs/src/engine/mod.rs")
engine = engine_path.read_text()
start_marker = indent(
    dedent(
        """\
        let (run_id, idempotent_replay) = if let Some(idempotency_key) =
            request.idempotency_key.as_deref()
        {
        """
    ),
    "        ",
)
end_marker = indent(
    dedent(
        """\
        } else {
            (Uuid::new_v4().to_string(), false)
        };
        """
    ),
    "        ",
)
start = engine.find(start_marker)
if start < 0:
    raise SystemExit("submit_run idempotency block start was not found")
end_start = engine.find(end_marker, start)
if end_start < 0:
    raise SystemExit("submit_run idempotency block end was not found")
end = end_start + len(end_marker)
replacement = indent(
    dedent(
        """\
        let (run_id, idempotent_replay) = if let Some(idempotency_key) =
            request.idempotency_key.as_deref()
        {
            let record_key = idempotency_key_key(idempotency_key);
            if let Some(existing) = self.load::<IdempotencyRecord>(&record_key).await? {
                if existing.value.request_hash != request_hash {
                    return Err(EngineError::IdempotencyMismatch);
                }
                if self
                    .load::<RunRecord>(&run_key(&existing.value.run_id))
                    .await?
                    .is_some()
                {
                    self.metrics
                        .idempotent_replays_total
                        .fetch_add(1, Ordering::Relaxed);
                    return self
                        .idempotent_submit_response(&existing.value.run_id)
                        .await;
                }
                // A process may stop after reserving the idempotency key but
                // before committing the deterministic run and step records.
                // Resume materialization rather than poisoning every retry.
                (existing.value.run_id, true)
            } else {
                let run_id =
                    Uuid::new_v5(&Uuid::NAMESPACE_URL, idempotency_key.as_bytes()).to_string();
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
                        if self
                            .load::<RunRecord>(&run_key(&existing.value.run_id))
                            .await?
                            .is_some()
                        {
                            self.metrics
                                .idempotent_replays_total
                                .fetch_add(1, Ordering::Relaxed);
                            return self
                                .idempotent_submit_response(&existing.value.run_id)
                                .await;
                        }
                        (existing.value.run_id, true)
                    }
                    Err(error) => return Err(error),
                }
            }
        } else {
            (Uuid::new_v4().to_string(), false)
        };
        """
    ),
    "        ",
)
engine_path.write_text(engine[:start] + replacement + engine[end:])

test_path = Path(
    "remote/deployments/durable-worker-server-rs/tests/idempotency_recovery.rs"
)
test_path.write_text(
    dedent(
        r"""\
        use std::{collections::BTreeSet, sync::Arc};

        use bytes::Bytes;
        use sha2::{Digest, Sha256};
        use uuid::Uuid;

        use dd_durable_worker_server::{
            engine::ManualClock,
            model::{
                IdempotencyRecord, JsonObject, RetryPolicy, StepDefinition, SubmitRunRequest,
            },
            Engine, MemoryEventSink, MemoryStore, StateStore,
        };

        fn recovery_step() -> StepDefinition {
            StepDefinition {
                key: "recover".to_string(),
                task_type: "agent:recover".to_string(),
                queue: "agents".to_string(),
                input: JsonObject::new(),
                depends_on: Vec::new(),
                priority: 0,
                required_capabilities: BTreeSet::new(),
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

        #[tokio::test]
        async fn resumes_submission_after_idempotency_record_was_committed_first() {
            let now_ms = 9_000_000;
            let store = Arc::new(MemoryStore::new());
            let engine = Engine::new(
                store.clone(),
                Arc::new(MemoryEventSink::new()),
                Arc::new(ManualClock::new(now_ms)),
            );
            let idempotency_key = "interrupted-before-run";
            let request = SubmitRunRequest {
                idempotency_key: Some(idempotency_key.to_string()),
                name: Some("resume materialization".to_string()),
                metadata: JsonObject::new(),
                steps: vec![recovery_step()],
            };
            let run_id =
                Uuid::new_v5(&Uuid::NAMESPACE_URL, idempotency_key.as_bytes()).to_string();
            let request_hash =
                hex::encode(Sha256::digest(serde_json::to_vec(&request).unwrap()));
            let record = IdempotencyRecord {
                run_id: run_id.clone(),
                request_hash,
                created_at_ms: now_ms,
            };
            let record_key = format!(
                "idempotency.{}",
                hex::encode(Sha256::digest(idempotency_key.as_bytes()))
            );
            store
                .create(
                    &record_key,
                    Bytes::from(serde_json::to_vec(&record).unwrap()),
                )
                .await
                .unwrap();

            let submitted = engine.submit_run(request).await.unwrap();
            assert_eq!(submitted.run_id, run_id);
            assert!(submitted.idempotent_replay);

            let snapshot = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
            assert_eq!(snapshot.run.counts.total, 1);
            assert_eq!(snapshot.steps.len(), 1);
            assert_eq!(snapshot.steps[0].key, "recover");
        }
        """
    )
)
