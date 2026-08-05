#[path = "../indie_dispatch.rs"]
mod indie_dispatch;

use super::{security, upstream, *};

struct PreparedBuildRequest {
    validated: ValidatedBuildRequest,
    upstream_request: Value,
}

pub(super) async fn submit_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(response) = security::require_auth(&headers, &state) {
        return *response;
    }
    if !state.config.execution_enabled {
        state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "executor routing is disabled",
                "hint": "enable only after at least one executor and its mounted credential have passed readiness and no-duplicate smoke tests"
            })),
        )
            .into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "build request must be valid JSON" })),
            )
                .into_response();
        }
    };
    let prepared = match prepare_build_request(&request) {
        Ok(prepared) => prepared,
        Err(error) => {
            state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": bounded_text(&error, state.config.max_error_chars)
                })),
            )
                .into_response();
        }
    };
    let PreparedBuildRequest {
        validated,
        upstream_request,
    } = prepared;
    info!(
        request_id = %validated.request_id,
        repository = %validated.repository,
        revision = %validated.revision,
        profile = %validated.profile,
        "validated immutable fixed-profile executor request"
    );

    if let Some(existing) = assignment_for(&state, &validated.request_id).await {
        return await_assignment(&state, existing, &validated).await;
    }

    let Some(executor) = upstream::first_ready_executor(&state).await.cloned() else {
        state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "no reviewed executor is ready",
                "retryable": true,
                "submissionAttempted": false
            })),
        )
            .into_response();
    };

    let assignment = Arc::new(Assignment::new(validated.clone(), executor.id.clone()));
    {
        let mut assignments = state.assignments.lock().await;
        if let Some(existing) = assignments.get(&validated.request_id).cloned() {
            drop(assignments);
            return await_assignment(&state, existing, &validated).await;
        }
        if assignments.len() >= state.config.max_assignments {
            state
                .metrics
                .assignment_capacity_exhausted_total
                .fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "executor assignment retention is full; refusing an untracked submission",
                    "retryable": false,
                    "submissionAttempted": false
                })),
            )
                .into_response();
        }
        assignments.insert(validated.request_id.clone(), assignment.clone());
    }

    upstream::record_selection(&state.metrics, executor.provider);
    state
        .metrics
        .submissions_total
        .fetch_add(1, Ordering::Relaxed);
    let task_state = state.clone();
    let task_assignment = assignment.clone();
    tokio::spawn(async move {
        let outcome = submit_to_executor(&task_state, &executor, &upstream_request).await;
        *task_assignment.outcome.lock().await = Some(outcome);
        task_assignment.notify.notify_waiters();
    });
    wait_for_outcome(assignment).await
}

fn prepare_build_request(request: &Value) -> Result<PreparedBuildRequest, String> {
    if let Some(adapted) = indie_dispatch::adapt_dispatch(request)? {
        return Ok(PreparedBuildRequest {
            validated: ValidatedBuildRequest {
                request_id: adapted.request_id,
                repository: adapted.repository,
                revision: adapted.revision,
                profile: adapted.profile,
            },
            upstream_request: adapted.upstream_request,
        });
    }

    Ok(PreparedBuildRequest {
        validated: validate_build_request(request)?,
        upstream_request: request.clone(),
    })
}

async fn assignment_for(state: &AppState, request_id: &str) -> Option<Arc<Assignment>> {
    state.assignments.lock().await.get(request_id).cloned()
}

async fn await_assignment(
    state: &AppState,
    assignment: Arc<Assignment>,
    request: &ValidatedBuildRequest,
) -> Response {
    if &assignment.request != request {
        state
            .metrics
            .duplicate_conflicts_total
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "requestId is already bound to a different immutable executor request",
                "executorId": assignment.executor_id.clone(),
                "automaticFailover": false
            })),
        )
            .into_response();
    }
    state
        .metrics
        .duplicate_hits_total
        .fetch_add(1, Ordering::Relaxed);
    wait_for_outcome(assignment).await
}

async fn wait_for_outcome(assignment: Arc<Assignment>) -> Response {
    loop {
        let notified = assignment.notify.notified();
        if let Some(outcome) = assignment.outcome.lock().await.clone() {
            return outcome_response(&assignment.executor_id, outcome);
        }
        notified.await;
    }
}

async fn submit_to_executor(
    state: &AppState,
    executor: &Executor,
    request: &Value,
) -> AssignmentOutcome {
    let response = match state
        .client
        .post(format!("{}/builds", executor.base_url))
        .header("x-build-server-auth", &executor.auth)
        .json(request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            state
                .metrics
                .ambiguous_submissions_total
                .fetch_add(1, Ordering::Relaxed);
            return AssignmentOutcome::Ambiguous {
                upstream_status: None,
            };
        }
    };
    let status = response.status();
    if status != StatusCode::ACCEPTED {
        if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
            state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
            return AssignmentOutcome::Rejected {
                status,
                body: json!({
                    "error": "selected executor rejected the fixed-profile request",
                    "executorId": executor.id.clone(),
                    "upstreamStatus": status.as_u16(),
                    "automaticFailover": false
                }),
            };
        }
        state
            .metrics
            .ambiguous_submissions_total
            .fetch_add(1, Ordering::Relaxed);
        return AssignmentOutcome::Ambiguous {
            upstream_status: Some(status.as_u16()),
        };
    }

    let body =
        match upstream::read_bounded_body(response, state.config.max_upstream_body_bytes).await {
            Ok(body) => body,
            Err(_) => {
                state
                    .metrics
                    .ambiguous_submissions_total
                    .fetch_add(1, Ordering::Relaxed);
                return AssignmentOutcome::Ambiguous {
                    upstream_status: Some(StatusCode::ACCEPTED.as_u16()),
                };
            }
        };
    let mut value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            state
                .metrics
                .ambiguous_submissions_total
                .fetch_add(1, Ordering::Relaxed);
            return AssignmentOutcome::Ambiguous {
                upstream_status: Some(StatusCode::ACCEPTED.as_u16()),
            };
        }
    };
    let Some(object) = value.as_object_mut() else {
        state
            .metrics
            .ambiguous_submissions_total
            .fetch_add(1, Ordering::Relaxed);
        return AssignmentOutcome::Ambiguous {
            upstream_status: Some(StatusCode::ACCEPTED.as_u16()),
        };
    };
    let Some(upstream_id) = object.get("id").and_then(Value::as_str) else {
        state
            .metrics
            .ambiguous_submissions_total
            .fetch_add(1, Ordering::Relaxed);
        return AssignmentOutcome::Ambiguous {
            upstream_status: Some(StatusCode::ACCEPTED.as_u16()),
        };
    };
    let route_id = match namespace_build_id(&executor.id, upstream_id) {
        Ok(route_id) => route_id,
        Err(_) => {
            state
                .metrics
                .ambiguous_submissions_total
                .fetch_add(1, Ordering::Relaxed);
            return AssignmentOutcome::Ambiguous {
                upstream_status: Some(StatusCode::ACCEPTED.as_u16()),
            };
        }
    };
    object.insert("id".into(), Value::String(route_id));
    object.insert("executorId".into(), Value::String(executor.id.clone()));
    object.insert(
        "provider".into(),
        Value::String(executor.provider.as_str().to_string()),
    );
    state
        .metrics
        .submissions_accepted_total
        .fetch_add(1, Ordering::Relaxed);
    AssignmentOutcome::Accepted(value)
}

fn outcome_response(executor_id: &str, outcome: AssignmentOutcome) -> Response {
    match outcome {
        AssignmentOutcome::Accepted(value) => (StatusCode::ACCEPTED, Json(value)).into_response(),
        AssignmentOutcome::Rejected { status, body } => (status, Json(body)).into_response(),
        AssignmentOutcome::Ambiguous { upstream_status } => {
            upstream::ambiguous_submission(executor_id, upstream_status.and_then(status_from_u16))
        }
    }
}

fn status_from_u16(value: u16) -> Option<StatusCode> {
    StatusCode::from_u16(value).ok()
}
