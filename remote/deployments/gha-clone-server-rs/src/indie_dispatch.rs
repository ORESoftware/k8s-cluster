//! Strict adapter from the planner/profile protocol into the legacy fixed-profile
//! build-server envelope.
//!
//! The adapter validates and recomputes every content-binding digest before the
//! executor router records an assignment. Orchestration-only metadata remains
//! bound into the request identity but is not exposed as worker commands,
//! environment variables, image names, or caller-selected executors.
//!
//! This module is loaded through the executor-router service's existing `#[path]`
//! module graph. Keep each child path explicit so every binary resolves the same
//! adapter sources instead of inheriting the including module's directory.

#[path = "indie_dispatch/digest.rs"]
mod digest;
#[path = "indie_dispatch/model.rs"]
mod model;
#[path = "indie_dispatch/validate/mod.rs"]
mod validate;

use serde_json::{json, Value};

use self::digest::{request_digest, stable_request_id};
use self::model::DispatchRequest;
use self::validate::validate_request;

const DISPATCH_SCHEMA: &str = "gha-indie-worker.dispatch.v1";

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

#[cfg(test)]
#[path = "indie_dispatch/tests.rs"]
mod tests;
