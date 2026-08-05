use oresoftware_durable_worker::Assignment;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    version: u32,
    delivery: String,
    effect_safety: Vec<String>,
    transient_statuses: Vec<u16>,
    lease_lost_statuses: Vec<u16>,
    never_retry_without_identity: Vec<String>,
    idempotent_operations: Vec<String>,
    endpoint_fragments: Vec<String>,
    progress_chunk_id: String,
    assignment: Assignment,
}

#[test]
fn consumes_the_shared_protocol_v1_fixture() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../fixtures/durable-worker-protocol-v1.json"
    ))
    .expect("shared protocol fixture");
    assert_eq!(fixture.version, 1);
    assert_eq!(fixture.delivery, "at-least-once");
    assert!(fixture
        .effect_safety
        .contains(&"idempotency-key".to_owned()));
    assert!(fixture.effect_safety.contains(&"fencing-token".to_owned()));
    assert_eq!(
        fixture.transient_statuses,
        vec![408, 425, 429, 500, 502, 503, 504]
    );
    assert_eq!(fixture.lease_lost_statuses, vec![404, 409]);
    assert!(fixture
        .never_retry_without_identity
        .contains(&"worker-poll".to_owned()));
    assert!(fixture
        .idempotent_operations
        .contains(&"complete-step-with-lease-generation".to_owned()));
    assert!(fixture
        .endpoint_fragments
        .contains(&"/api/v1/tasks".to_owned()));
    assert_eq!(
        fixture.progress_chunk_id,
        "{stepId}:{leaseGeneration}:{sequence}"
    );
    assert_eq!(fixture.assignment.step_id, "step-fixture");
    assert_eq!(fixture.assignment.lease_generation, 3);
    assert_eq!(fixture.assignment.fencing_token, 9);
}
