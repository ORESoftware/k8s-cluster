use super::{security, *};

pub(super) async fn get_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(route_id): AxumPath<String>,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .status_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = security::require_auth(&headers, &state) {
        return response;
    }
    let (executor_id, upstream_id) = match parse_namespaced_build_id(&route_id) {
        Ok(parts) => parts,
        Err(error) => {
            state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    let Some(executor) = state
        .config
        .executors
        .iter()
        .find(|executor| executor.id == executor_id)
    else {
        state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "executor route is not configured" })),
        )
            .into_response();
    };
    let response = match state
        .client
        .get(format!("{}/builds/{upstream_id}", executor.base_url))
        .header("x-build-server-auth", &executor.auth)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "status request to the accepting executor failed",
                    "executorId": executor.id,
                    "automaticFailover": false
                })),
            )
                .into_response()
        }
    };
    let status = response.status();
    if status != StatusCode::OK {
        return (
            if status == StatusCode::NOT_FOUND {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_GATEWAY
            },
            Json(json!({
                "error": "accepting executor did not return build status",
                "executorId": executor.id,
                "upstreamStatus": status.as_u16(),
                "automaticFailover": false
            })),
        )
            .into_response();
    }
    let body = match read_bounded_body(response, state.config.max_upstream_body_bytes).await {
        Ok(body) => body,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "accepting executor returned an invalid bounded status response",
                    "executorId": executor.id
                })),
            )
                .into_response()
        }
    };
    let mut value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "accepting executor returned invalid status JSON",
                    "executorId": executor.id
                })),
            )
                .into_response()
        }
    };
    let Some(object) = value.as_object_mut() else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "accepting executor returned a non-object status",
                "executorId": executor.id
            })),
        )
            .into_response();
    };
    if object.get("id").and_then(Value::as_str) != Some(upstream_id) {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "accepting executor returned a mismatched build id",
                "executorId": executor.id
            })),
        )
            .into_response();
    }
    object.insert("id".into(), Value::String(route_id));
    object.insert("executorId".into(), Value::String(executor.id.clone()));
    object.insert(
        "provider".into(),
        Value::String(executor.provider.as_str().to_string()),
    );
    (StatusCode::OK, Json(value)).into_response()
}

pub(super) async fn first_ready_executor(state: &AppState) -> Option<&Executor> {
    for executor in &state.config.executors {
        if executor_ready(state, executor).await {
            return Some(executor);
        }
    }
    None
}

pub(super) async fn executor_ready(state: &AppState, executor: &Executor) -> bool {
    match state
        .client
        .get(format!("{}/readyz", executor.base_url))
        .timeout(state.config.probe_timeout)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => true,
        Ok(_) | Err(_) => {
            state
                .metrics
                .readiness_failures_total
                .fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

pub(super) fn record_selection(metrics: &Metrics, provider: Provider) {
    match provider {
        Provider::Aws => metrics.aws_selections_total.fetch_add(1, Ordering::Relaxed),
        Provider::Hetzner => metrics
            .hetzner_selections_total
            .fetch_add(1, Ordering::Relaxed),
    };
}

pub(super) fn ambiguous_submission(executor_id: &str, status: Option<StatusCode>) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": "executor submission outcome is ambiguous; automatic provider failover is blocked to prevent duplicate work",
            "executorId": executor_id,
            "upstreamStatus": status.map(|value| value.as_u16()),
            "automaticFailover": false,
            "retryGuidance": "reconcile the deterministic requestId through shared build-server/Fiducia state before any operator retry"
        })),
    )
        .into_response()
}

pub(super) async fn read_bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err("upstream response exceeds configured body bound".to_string());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "upstream response body could not be read".to_string())?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err("upstream response exceeds configured body bound".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
