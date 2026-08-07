
async fn github_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.counters.received.fetch_add(1, Ordering::Relaxed);

    let Some(signature) = header_value(&headers, "x-hub-signature-256") else {
        return reject(&state, StatusCode::UNAUTHORIZED, "missing signature");
    };
    if !verify_github_signature(&state.config.webhook_secret, &body, signature) {
        return reject(&state, StatusCode::UNAUTHORIZED, "invalid signature");
    }

    let Some(delivery) = header_value(&headers, "x-github-delivery") else {
        return reject(&state, StatusCode::BAD_REQUEST, "missing delivery id");
    };
    if !valid_delivery(delivery) {
        return reject(&state, StatusCode::BAD_REQUEST, "invalid delivery id");
    }

    let event_name = header_value(&headers, "x-github-event").unwrap_or_default();
    if event_name == "ping" {
        return (StatusCode::OK, Json(json!({ "ok": true, "action": "pong" }))).into_response();
    }
    if event_name != "workflow_run" {
        state.counters.ignored.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::OK, Json(json!({ "ok": true, "action": "ignored" }))).into_response();
    }

    {
        let mut deliveries = state.deliveries.lock().await;
        if !deliveries.insert(delivery) {
            state.counters.duplicates.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::OK,
                Json(json!({ "ok": true, "action": "duplicate", "delivery": delivery })),
            )
                .into_response();
        }
    }

    let event: WorkflowRunEvent = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            state.deliveries.lock().await.remove(delivery);
            return reject(&state, StatusCode::BAD_REQUEST, "invalid workflow_run payload");
        }
    };

    if event.action != "completed"
        || !valid_commit_sha(&event.workflow_run.head_sha)
        || event.workflow_run.head_branch.is_none()
        || event.workflow_run.conclusion.is_none()
    {
        state.counters.ignored.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::OK, Json(json!({ "ok": true, "action": "ignored" }))).into_response();
    }

    let Some(head_repository) = event.workflow_run.head_repository.as_ref() else {
        state.counters.ignored.fetch_add(1, Ordering::Relaxed);
        warn!(
            repository = %event.repository.full_name,
            source_run_id = event.workflow_run.id,
            "ignored workflow_run without head repository"
        );
        return (
            StatusCode::OK,
            Json(json!({ "ok": true, "action": "ignored-head-repository" })),
        )
            .into_response();
    };
    if !head_repository
        .full_name
        .eq_ignore_ascii_case(&event.repository.full_name)
    {
        state.counters.ignored.fetch_add(1, Ordering::Relaxed);
        warn!(
            repository = %event.repository.full_name,
            source_run_id = event.workflow_run.id,
            "ignored fork-originated workflow_run"
        );
        return (
            StatusCode::OK,
            Json(json!({ "ok": true, "action": "ignored-fork" })),
        )
            .into_response();
    }

    let Some(rule) = select_rule(&event, &state.config.rules) else {
        state.counters.ignored.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::OK, Json(json!({ "ok": true, "action": "ignored" }))).into_response();
    };

    let action_name = match &rule.action {
        RuleAction::WorkflowDispatch { .. } => "workflow-dispatch",
        RuleAction::BuildServerProfile { .. } => "build-server-profile",
    };

    if state.config.dry_run {
        state.counters.dispatched.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::ACCEPTED,
            Json(Receipt {
                accepted: true,
                action: action_name,
                delivery: delivery.to_string(),
                repository: event.repository.full_name,
                source_run_id: event.workflow_run.id,
                dry_run: true,
            }),
        )
            .into_response();
    }

    let result = match &rule.action {
        RuleAction::WorkflowDispatch {
            workflow_file,
            dispatch_ref,
            runner,
            extra_inputs,
            ..
        } => {
            dispatch_workflow(
                &state,
                &event,
                delivery,
                workflow_file,
                dispatch_ref,
                runner,
                extra_inputs,
            )
            .await
        }
        RuleAction::BuildServerProfile { profile, executor } => {
            dispatch_build_profile(&state, &event, delivery, profile, executor.as_deref()).await
        }
    };

    match result {
        Ok(()) => {
            state.counters.dispatched.fetch_add(1, Ordering::Relaxed);
            info!(
                repository = %event.repository.full_name,
                source_run_id = event.workflow_run.id,
                action = action_name,
                "fallback accepted"
            );
            (
                StatusCode::ACCEPTED,
                Json(Receipt {
                    accepted: true,
                    action: action_name,
                    delivery: delivery.to_string(),
                    repository: event.repository.full_name,
                    source_run_id: event.workflow_run.id,
                    dry_run: false,
                }),
            )
                .into_response()
        }
        Err(error_message) => {
            state.counters.failed.fetch_add(1, Ordering::Relaxed);
            state.deliveries.lock().await.remove(delivery);
            error!(
                repository = %event.repository.full_name,
                source_run_id = event.workflow_run.id,
                action = action_name,
                error = %error_message,
                "fallback dispatch failed"
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "fallback dispatch failed" })),
            )
                .into_response()
        }
    }
}

async fn dispatch_workflow(
    state: &AppState,
    event: &WorkflowRunEvent,
    delivery: &str,
    workflow_file: &str,
    dispatch_ref: &str,
    runner: &str,
    extra_inputs: &HashMap<String, String>,
) -> Result<(), String> {
    let token = state
        .config
        .github_token
        .as_deref()
        .ok_or_else(|| "GitHub token is not configured".to_string())?;
    let branch = event
        .workflow_run
        .head_branch
        .as_deref()
        .ok_or_else(|| "source branch is missing".to_string())?;
    let mut inputs = extra_inputs.clone();
    inputs.insert("source_repository".to_string(), event.repository.full_name.clone());
    inputs.insert("source_ref".to_string(), branch.to_string());
    inputs.insert("source_sha".to_string(), event.workflow_run.head_sha.clone());
    inputs.insert("source_workflow".to_string(), event.workflow_run.name.clone());
    inputs.insert("source_run_id".to_string(), event.workflow_run.id.to_string());
    inputs.insert(
        "source_run_attempt".to_string(),
        event.workflow_run.run_attempt.max(1).to_string(),
    );
    inputs.insert("runner".to_string(), runner.to_string());
    inputs.insert("webhook_delivery".to_string(), delivery.to_string());

    let url = format!(
        "https://api.github.com/repos/{}/actions/workflows/{}/dispatches",
        event.repository.full_name, workflow_file
    );
    let response = state
        .http
        .post(url)
        .bearer_auth(token)
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", GITHUB_API_VERSION)
        .header("user-agent", SERVICE_NAME)
        .json(&json!({ "ref": dispatch_ref, "inputs": inputs }))
        .send()
        .await
        .map_err(|error| format!("GitHub dispatch request failed: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "GitHub dispatch returned status {}",
            response.status().as_u16()
        ))
    }
}

async fn dispatch_build_profile(
    state: &AppState,
    event: &WorkflowRunEvent,
    delivery: &str,
    profile: &str,
    executor: Option<&str>,
) -> Result<(), String> {
    let auth = state
        .config
        .build_server_auth
        .as_deref()
        .ok_or_else(|| "build-server auth is not configured".to_string())?;
    let branch = event
        .workflow_run
        .head_branch
        .as_deref()
        .ok_or_else(|| "source branch is missing".to_string())?;
    let mut request = json!({
        "schemaVersion": "build-server.v1",
        "jobKind": "run-profile",
        "repoUrl": format!("https://github.com/{}.git", event.repository.full_name),
        "gitRef": branch,
        "profile": profile,
        "requestId": format!("gha-fallback:{}:{}", delivery, event.workflow_run.head_sha),
    });
    if let Some(executor) = executor {
        request["executor"] = Value::String(executor.to_string());
    }

    let response = state
        .http
        .post(format!("{}/builds", state.config.build_server_url))
        .header("x-server-auth", auth)
        .json(&request)
        .send()
        .await
        .map_err(|error| format!("build-server request failed: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "build-server returned status {}",
            response.status().as_u16()
        ))
    }
}
