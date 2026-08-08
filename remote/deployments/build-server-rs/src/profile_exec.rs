use std::{path::Path, time::Duration};

use serde_json::{json, Value};
use tokio::time::timeout;

use crate::exec::append_log;
use crate::state::AppState;
use crate::types::{BuildJobRecord, BuildRequest};

const RUNNER_URL: &str = "http://dd-ci-profile-runner.default.svc.cluster.local:8147/run";
const SCHEMA: &str = "ci-profile-runner.v1";
const PLAYWRIGHT_REPOSITORY: &str =
    "https://github.com/discrete-event-systems-test/des-web-playwright-e2e.git";
const PUPPETEER_REPOSITORY: &str =
    "https://github.com/discrete-event-systems-test/des-web-puppeteer-e2e.git";

fn exact_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn exact_repository(repo_url: &str, profile: &str) -> Option<&'static str> {
    match (repo_url, profile) {
        (PLAYWRIGHT_REPOSITORY, "playwright") => {
            Some("discrete-event-systems-test/des-web-playwright-e2e")
        }
        (PUPPETEER_REPOSITORY, "puppeteer") => {
            Some("discrete-event-systems-test/des-web-puppeteer-e2e")
        }
        _ => None,
    }
}

pub(crate) fn should_delegate(request: &BuildRequest, profile: &str) -> bool {
    exact_repository(&request.repo_url, profile).is_some()
        && request.git_ref.as_deref().is_some_and(exact_commit)
}

pub(crate) async fn execute(
    state: &AppState,
    job: &BuildJobRecord,
    profile: &str,
    log_path: &Path,
) -> Result<(), String> {
    let request = &job.request;
    let repository = exact_repository(&request.repo_url, profile)
        .ok_or_else(|| "CI profile runner delegation lost its exact repository binding".to_string())?;
    let revision = request
        .git_ref
        .as_deref()
        .filter(|value| exact_commit(value))
        .ok_or_else(|| "CI profile runner delegation requires an exact 40-hex revision".to_string())?;
    let auth = state
        .config
        .server_auth_secret
        .as_deref()
        .ok_or_else(|| "CI profile runner delegation requires build-server auth".to_string())?;

    append_log(
        log_path,
        &format!(
            "delegating exact profile job={} repository={} revision={} profile={} runner={}\n",
            job.id, repository, revision, profile, RUNNER_URL
        ),
        state.config.max_log_bytes,
    )
    .await;

    let body = json!({
        "schemaVersion": SCHEMA,
        "requestId": format!("build:{}", job.id),
        "repository": repository,
        "revision": revision,
        "profile": profile,
    });
    let send = state
        .http
        .post(RUNNER_URL)
        .header("x-server-auth", auth)
        .json(&body)
        .send();
    let response = timeout(
        state.config.job_deadline.max(Duration::from_secs(60)),
        send,
    )
    .await
    .map_err(|_| "CI profile runner request timed out".to_string())?
    .map_err(|error| format!("CI profile runner request failed: {error}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("CI profile runner response read failed: {error}"))?;
    if bytes.len() > 256 * 1024 {
        return Err("CI profile runner response exceeded 256 KiB".to_string());
    }
    let payload: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "CI profile runner returned invalid JSON".to_string())?;

    if let Some(output) = payload.get("outputTail").and_then(Value::as_str) {
        append_log(
            log_path,
            &format!("\nci-profile-runner output tail:\n{output}\n"),
            state.config.max_log_bytes,
        )
        .await;
    }

    let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if !status.is_success() || !ok {
        let error = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("fixed profile runner rejected or failed the request");
        return Err(format!(
            "CI profile runner failed with HTTP {}: {error}",
            status.as_u16()
        ));
    }

    let observed_repository = payload
        .get("repository")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let observed_revision = payload
        .get("revision")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let observed_profile = payload
        .get("profile")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if observed_repository != repository
        || observed_revision != revision
        || observed_profile != profile
    {
        return Err("CI profile runner success evidence did not match the submitted identity".to_string());
    }

    append_log(
        log_path,
        &format!(
            "CI profile runner completed exact profile job={} repository={} revision={} profile={}\n",
            job.id, repository, revision, profile
        ),
        state.config.max_log_bytes,
    )
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(repo_url: &str, git_ref: &str) -> BuildRequest {
        BuildRequest {
            schema_version: Some("build-server.v1".to_string()),
            job_kind: Some("run-profile".to_string()),
            repo_url: repo_url.to_string(),
            git_ref: Some(git_ref.to_string()),
            image: String::new(),
            profile: Some("playwright".to_string()),
            context_dir: Some(".".to_string()),
            dockerfile: None,
            build_args: None,
            push: Some(false),
            deploy: None,
            executor: Some("local".to_string()),
            request_id: None,
        }
    }

    #[test]
    fn only_exact_des_browser_bindings_delegate() {
        let sha = "1e1116ef6811c4e3e6be34ad3e1def39bc20ef59";
        assert!(should_delegate(
            &request(PLAYWRIGHT_REPOSITORY, sha),
            "playwright"
        ));
        assert!(!should_delegate(
            &request(PLAYWRIGHT_REPOSITORY, "main"),
            "playwright"
        ));
        assert!(!should_delegate(
            &request(
                "https://github.com/discrete-event-systems-test/des-web-playwright-e2e-lookalike.git",
                sha,
            ),
            "playwright"
        ));
        assert!(!should_delegate(
            &request(PLAYWRIGHT_REPOSITORY, sha),
            "puppeteer"
        ));
    }
}
