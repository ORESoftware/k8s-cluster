//! GitHub webhook handler.
//!
//! Verifies the HMAC signature, parses the event, applies the
//! repo-allowlist / dedupe / path-filter checks, and dispatches a
//! background analysis task. The HTTP response returns immediately (202
//! Accepted) with the assigned `analysis_id` so the caller can correlate
//! logs.

use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use bytes::Bytes;
use serde_json::json;
use tracing::{info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::analysis::workspace::CheckoutSpec;
use crate::error::AppError;
use crate::github::client::CommitStatusState;
use crate::github::{PullRequestAction, PullRequestEvent};
use crate::signature::verify_github_signature;
use crate::state::AppState;

const SIG_HEADER: &str = "x-hub-signature-256";
const EVENT_HEADER: &str = "x-github-event";
const DELIVERY_HEADER: &str = "x-github-delivery";

pub async fn github(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let signature = required_header(&headers, SIG_HEADER)?;
    let event = required_header(&headers, EVENT_HEADER)?;
    let delivery = headers
        .get(DELIVERY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if !verify_github_signature(
        state.config.github_webhook_secret.as_bytes(),
        &body,
        &signature,
    ) {
        warn!(delivery, "rejecting webhook with bad signature");
        return Err(AppError::InvalidSignature);
    }

    if event == "ping" {
        info!(delivery, "received ping");
        return Ok((
            StatusCode::OK,
            Json(json!({ "status": "pong", "delivery": delivery })),
        ));
    }

    if event != "pull_request" {
        info!(event = %event, delivery, "ignoring unsupported event");
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "status": "ignored",
                "reason": "unsupported_event",
                "event": event,
                "delivery": delivery,
            })),
        ));
    }

    let payload: PullRequestEvent = serde_json::from_slice(&body)
        .map_err(|e| AppError::MalformedPayload(format!("pull_request payload: {e}")))?;

    if !payload.action.should_analyze() {
        info!(
            action = ?payload.action,
            delivery,
            pr = payload.number,
            "ignoring pull_request action"
        );
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "status": "ignored",
                "reason": "action_not_analyzable",
                "action": action_as_str(payload.action),
                "delivery": delivery,
            })),
        ));
    }

    // Repo allowlist: defence-in-depth against a leaked webhook secret being
    // pointed at unrelated repos. Configured via FORMAL_METHODS_ALLOWED_REPOS.
    if !state.repo_allowlist.allows(&payload.repository.full_name) {
        warn!(
            delivery,
            repo = %payload.repository.full_name,
            "rejecting pull_request: repo not in allowlist"
        );
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "status": "ignored",
                "reason": "repo_not_allowed",
                "repo": payload.repository.full_name,
                "delivery": delivery,
            })),
        ));
    }

    // Drafts are skipped except when the action is `ready_for_review`.
    if matches!(payload.pull_request.draft, Some(true))
        && payload.action != PullRequestAction::ReadyForReview
    {
        info!(delivery, pr = payload.number, "ignoring draft pull_request");
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "status": "ignored",
                "reason": "draft",
                "delivery": delivery,
            })),
        ));
    }

    // Dedupe on X-GitHub-Delivery so GitHub's automatic retries do not
    // duplicate analyses. The "unknown" sentinel is intentionally never
    // deduped: missing the header (e.g. hand-rolled tooling) should still
    // run.
    if delivery != "unknown" {
        let mut guard = state
            .delivery_dedupe
            .lock()
            .map_err(|_| AppError::Internal("delivery dedupe mutex poisoned".into()))?;
        let fresh = guard.record(&delivery);
        drop(guard);
        if !fresh {
            info!(delivery, pr = payload.number, "duplicate delivery ignored");
            return Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "status": "duplicate",
                    "delivery": delivery,
                })),
            ));
        }
    }

    // Path filter: when configured, fetch the PR's changed file list and
    // skip the pipeline if none of them are in scope. We post a success
    // status anyway so branch protection passes. Skipping is conservative:
    // any I/O error from GitHub leads to running the pipeline (open-failed).
    if !state.path_filter.is_empty() {
        match state
            .github
            .list_pull_request_files(
                &payload.repository.full_name,
                payload.number,
                state.config.max_pr_files_pages,
            )
            .await
        {
            Ok((files, truncated)) => {
                let in_scope = state.path_filter.matches_any(&files);
                if !in_scope && !truncated {
                    let head_sha = payload.pull_request.head.sha.clone();
                    let target_url = payload.pull_request.html_url.clone();
                    let repo_full_name = payload.repository.full_name.clone();
                    let github = state.github.clone();
                    let status_context = state.config.status_context.clone();
                    tokio::spawn(async move {
                        if let Err(err) = github
                            .post_commit_status(
                                &repo_full_name,
                                &head_sha,
                                CommitStatusState::Success,
                                &status_context,
                                "skipped: PR does not touch contract code",
                                target_url.as_deref(),
                            )
                            .await
                        {
                            warn!(error = %err, "failed to post skip status");
                        }
                    });
                    info!(
                        delivery,
                        pr = payload.number,
                        files = files.len(),
                        "no contract changes; skipping pipeline"
                    );
                    return Ok((
                        StatusCode::ACCEPTED,
                        Json(json!({
                            "status": "skipped",
                            "reason": "no_contract_changes",
                            "delivery": delivery,
                            "files_checked": files.len(),
                        })),
                    ));
                }
                if truncated {
                    info!(
                        delivery,
                        pr = payload.number,
                        "PR file list truncated; running pipeline conservatively"
                    );
                }
            }
            Err(err) => {
                // Network error / 5xx: fall through to running the pipeline.
                warn!(error = %err, delivery, pr = payload.number, "could not list PR files; running pipeline");
            }
        }
    }

    let analysis_id = Uuid::new_v4().to_string();

    info!(
        analysis_id,
        delivery,
        repo = %payload.repository.full_name,
        pr = payload.number,
        sha = %payload.pull_request.head.sha,
        action = ?payload.action,
        "accepted pull_request for analysis"
    );

    spawn_analysis(state.clone(), payload, analysis_id.clone()).await;

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "analysis_id": analysis_id,
            "delivery": delivery,
        })),
    ))
}

async fn spawn_analysis(state: AppState, event: PullRequestEvent, analysis_id: String) {
    let analyzer_timeout = state.config.analyzer_timeout;
    let workdir_root = state.config.workdir_root.clone();
    let status_context = state.config.status_context.clone();
    let span_id = analysis_id.clone();

    let clone_url = event
        .pull_request
        .head
        .repo
        .as_ref()
        .and_then(|r| r.clone_url.clone())
        .or_else(|| event.repository.clone_url.clone())
        .unwrap_or_else(|| format!("https://github.com/{}.git", event.repository.full_name));

    let clone_url = state.github.authenticated_clone_url(&clone_url);
    let head_sha = event.pull_request.head.sha.clone();
    let head_ref = event.pull_request.head.ref_name.clone();
    let repo_full_name = event.repository.full_name.clone();
    let target_url = event.pull_request.html_url.clone();

    tokio::spawn(
        async move {
            let _permit = match state.analysis_semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    warn!(analysis_id, "semaphore closed; skipping analysis");
                    return;
                }
            };

            let pending = state
                .github
                .post_commit_status(
                    &repo_full_name,
                    &head_sha,
                    CommitStatusState::Pending,
                    &status_context,
                    "Running formal-methods analysis",
                    target_url.as_deref(),
                )
                .await;
            if let Err(err) = pending {
                warn!(analysis_id, error = %err, "failed to post pending status");
            }

            let checkout_spec = CheckoutSpec {
                clone_url,
                head_sha: head_sha.clone(),
                head_ref,
                repo_full_name: repo_full_name.clone(),
                workdir_root,
                clone_timeout: Duration::from_secs(300).min(analyzer_timeout),
            };

            match state.pipeline.run(checkout_spec).await {
                Ok(report) => {
                    let overall = report.overall();
                    let description = report.description();
                    let github_state = match overall {
                        crate::analysis::StepStatus::Passed => CommitStatusState::Success,
                        crate::analysis::StepStatus::Failed => CommitStatusState::Failure,
                        crate::analysis::StepStatus::Skipped => CommitStatusState::Success,
                    };
                    info!(
                        analysis_id,
                        status = ?overall,
                        steps = report.steps.len(),
                        "analysis completed"
                    );
                    if let Err(err) = state
                        .github
                        .post_commit_status(
                            &repo_full_name,
                            &head_sha,
                            github_state,
                            &status_context,
                            &description,
                            target_url.as_deref(),
                        )
                        .await
                    {
                        warn!(analysis_id, error = %err, "failed to post final status");
                    }
                }
                Err(err) => {
                    warn!(analysis_id, error = %err, "analysis pipeline errored");
                    if let Err(post_err) = state
                        .github
                        .post_commit_status(
                            &repo_full_name,
                            &head_sha,
                            CommitStatusState::Error,
                            &status_context,
                            "Formal methods analysis errored",
                            target_url.as_deref(),
                        )
                        .await
                    {
                        warn!(analysis_id, error = %post_err, "failed to post error status");
                    }
                }
            }
        }
        .instrument(info_span!("analysis_task", analysis_id = %span_id)),
    );
}

fn required_header(headers: &HeaderMap, name: &'static str) -> Result<String, AppError> {
    let value = headers
        .get(name)
        .ok_or(AppError::MissingHeader(name))?
        .to_str()
        .map_err(|_| AppError::InvalidHeader(name))?;
    Ok(value.to_string())
}

fn action_as_str(action: PullRequestAction) -> &'static str {
    // Mirror the serde rename_all = "snake_case" mapping. Kept explicit so
    // changes to PullRequestAction don't silently drift.
    match action {
        PullRequestAction::Opened => "opened",
        PullRequestAction::Reopened => "reopened",
        PullRequestAction::Synchronize => "synchronize",
        PullRequestAction::ReadyForReview => "ready_for_review",
        PullRequestAction::Edited => "edited",
        PullRequestAction::Closed => "closed",
        PullRequestAction::Assigned => "assigned",
        PullRequestAction::Unassigned => "unassigned",
        PullRequestAction::Labeled => "labeled",
        PullRequestAction::Unlabeled => "unlabeled",
        PullRequestAction::ReviewRequested => "review_requested",
        PullRequestAction::ReviewRequestRemoved => "review_request_removed",
        PullRequestAction::ConvertedToDraft => "converted_to_draft",
        PullRequestAction::Locked => "locked",
        PullRequestAction::Unlocked => "unlocked",
        PullRequestAction::Milestoned => "milestoned",
        PullRequestAction::Demilestoned => "demilestoned",
        PullRequestAction::AutoMergeEnabled => "auto_merge_enabled",
        PullRequestAction::AutoMergeDisabled => "auto_merge_disabled",
        PullRequestAction::Dequeued => "dequeued",
        PullRequestAction::Enqueued => "enqueued",
        PullRequestAction::Other => "other",
    }
}
