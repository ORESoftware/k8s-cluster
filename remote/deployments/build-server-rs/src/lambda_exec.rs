//! Optional build executor backed by the gleam-lambda-runner service
//! (Scintilla). When a job sets `executor: "lambda"`, the validated
//! build-server.v1 document is forwarded to a pre-registered build function:
//!
//!   POST {BUILD_SERVER_LAMBDA_URL}/invoke/{BUILD_SERVER_LAMBDA_FUNCTION_ID}
//!
//! The lambda runner never receives arbitrary shell — it resolves its own
//! stored, sandboxed function definition and only gets the job JSON as input.
//! Auth reuses the shared dd-agent-secrets SERVER_AUTH_SECRET (the runner
//! accepts X-Server-Auth), overridable with BUILD_SERVER_LAMBDA_AUTH_SECRET.
//!
//! The full allowlist validation (repo/image/namespace) runs before this
//! executor is invoked, so lambda-executed jobs obey the same policy as
//! local nerdctl builds.

use serde_json::json;
use std::path::Path;

use crate::{append_log, AppState, BuildJobRecord};

pub async fn execute(state: &AppState, job: &BuildJobRecord, log_path: &Path) -> Result<(), String> {
    let config = &state.config;
    if !config.lambda_executor_enabled {
        return Err("lambda executor is disabled by BUILD_SERVER_LAMBDA_ENABLED=false".to_string());
    }
    let function_id = config
        .lambda_function_id
        .as_deref()
        .ok_or_else(|| "BUILD_SERVER_LAMBDA_FUNCTION_ID is not configured".to_string())?;
    let auth_secret = config
        .lambda_auth_secret
        .as_deref()
        .or(config.server_auth_secret.as_deref())
        .ok_or_else(|| "no auth secret available for the lambda runner".to_string())?;

    let url = format!(
        "{}/invoke/{function_id}",
        config.lambda_url.trim_end_matches('/')
    );
    append_log(
        log_path,
        &format!("dispatching job {} to gleam-lambda-runner function {function_id}\n", job.id),
        config.max_log_bytes,
    )
    .await;

    let payload = json!({
        "schemaVersion": "build-server.v1",
        "jobId": job.id,
        "request": job.request,
        "fencingToken": job.fencing_token,
    });
    let response = state
        .http
        .post(&url)
        .header("x-server-auth", auth_secret)
        .timeout(config.job_timeout)
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("lambda runner request failed: {error}"))?;

    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
    let output = body
        .get("output")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if !output.is_empty() {
        append_log(
            log_path,
            &format!("lambda output:\n{output}\n"),
            config.max_log_bytes,
        )
        .await;
    }
    let ok = body.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
    if status.is_success() && ok {
        Ok(())
    } else {
        let error = body
            .get("error")
            .map(|value| value.to_string())
            .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
        Err(format!("lambda build failed: {error}"))
    }
}
