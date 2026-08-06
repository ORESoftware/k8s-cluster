use std::{collections::BTreeSet, sync::Arc};

use bytes::Bytes;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use dd_durable_worker_server::{
    engine::ManualClock,
    model::{IdempotencyRecord, JsonObject, RetryPolicy, StepDefinition, SubmitRunRequest},
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
        deadline_ms: None,
        idempotency_key: Some(idempotency_key.to_string()),
        name: Some("resume materialization".to_string()),
        metadata: JsonObject::new(),
        steps: vec![recovery_step()],
    };
    let run_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, idempotency_key.as_bytes()).to_string();
    let request_hash = hex::encode(Sha256::digest(serde_json::to_vec(&request).unwrap()));
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
