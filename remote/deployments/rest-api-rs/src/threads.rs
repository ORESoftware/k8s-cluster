use std::env;

use axum::{
    body::Body,
    extract::Path,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use dd_nats_subject_defs::RUNTIME_EVENTS_SUBJECT;
use serde_json::{json, Value};

use crate::db::fetch_thread_repo_config_from_postgres;
use crate::events::publish_thread_runtime_event_to_nats;
use crate::k8s::{
    k8s_create_request, k8s_get_value, k8s_json_request, prune_awake_thread_workers_for_capacity,
    summarize_deployment, summarize_pod, summarize_service, summarize_thread_runtime,
};
use crate::metrics::record_request;
use crate::shared::{
    authorized_internal_request, json_at, looks_like_uuid, missing_worker_auth_secret_message,
    now_ms, public_thread_worker_proxy_error, unauthorized_response, worker_auth_secret,
};
use crate::types::{
    ThreadActionResponse, ThreadActionResult, ThreadControlRequest, ThreadRuntimeResponse,
};

pub(crate) fn thread_short_id(thread_id: &str) -> String {
    thread_id
        .chars()
        .filter(|value| value.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>()
        .to_lowercase()
}

pub(crate) fn thread_resource_name(thread_id: &str) -> String {
    format!("dd-thread-{}", thread_short_id(thread_id))
}

pub(crate) fn thread_terminal_url(thread_id: &str) -> String {
    format!(
        "/dd-thread/{}/terminal?threadId={thread_id}",
        thread_short_id(thread_id)
    )
}

pub(crate) fn validate_thread_control_signal(
    path_thread_id: &str,
    expected_action: &str,
    request: &ThreadControlRequest,
) -> Result<(), String> {
    if request.kind != "thread-control" {
        return Err("control payload kind must be thread-control".to_string());
    }
    if !looks_like_uuid(path_thread_id) {
        return Err("threadId must be a UUID".to_string());
    }
    if request.action != expected_action {
        return Err(format!("control payload action must be {expected_action}"));
    }
    if request.thread_id != path_thread_id {
        return Err("threadId path/body mismatch".to_string());
    }
    if let Some(task_id) = request.task_id.as_deref() {
        if !looks_like_uuid(task_id) {
            return Err("taskId must be a UUID".to_string());
        }
    }
    Ok(())
}

pub(crate) fn thread_runtime_namespace() -> String {
    env::var("THREAD_RUNTIME_NAMESPACE").unwrap_or_else(|_| "default".to_string())
}

pub(crate) fn thread_runtime_image() -> String {
    env::var("THREAD_RUNTIME_IMAGE")
        .unwrap_or_else(|_| "docker.io/library/dd-dev-server:dev".to_string())
}

pub(crate) fn thread_worker_url(namespace: &str, name: &str, path: &str) -> String {
    format!("http://{name}.{namespace}.svc.cluster.local:8080{path}")
}

pub(crate) fn render_thread_service(namespace: &str, name: &str, thread_id: &str) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/part-of": "dd-remote-dev",
                "app.kubernetes.io/component": "thread-pod",
                "dd/threadId": thread_id
            }
        },
        "spec": {
            "type": "ClusterIP",
            "selector": { "dd/threadId": thread_id },
            "ports": [{ "name": "http", "port": 8080, "targetPort": "http" }]
        }
    })
}

pub(crate) fn render_thread_deployment(
    namespace: &str,
    name: &str,
    thread_id: &str,
    repo_url: &str,
    base_branch: &str,
    thread_title: Option<&str>,
) -> Value {
    let image = thread_runtime_image();
    let mut env = vec![
        json!({ "name": "REMOTE_DEV_THREAD_ID", "value": thread_id }),
        json!({ "name": "DD_REPO_URL", "value": repo_url }),
        json!({ "name": "BASE_BRANCH", "value": base_branch }),
        json!({ "name": "IDLE_TIMEOUT_MS", "value": "0" }),
        json!({ "name": "OTEL_SERVICE_NAME", "value": name }),
        json!({ "name": "OTEL_EXPORTER_OTLP_ENDPOINT", "value": "http://dd-otel-collector.observability.svc.cluster.local:4318" }),
        json!({ "name": "THREAD_CONTEXT_BASE_URL", "value": "http://dd-remote-rest-api.default.svc.cluster.local:8082" }),
        json!({ "name": "AGENT_MCP_URL", "value": "http://dd-cluster-mcp-rs.default.svc.cluster.local:8091/mcp" }),
        json!({ "name": "AGENT_MCP_CONNECT_TIMEOUT_MS", "value": "3000" }),
        json!({ "name": "EVENT_INGEST_URL", "value": "http://dd-remote-rest-api.default.svc.cluster.local:8082/api/agents/events" }),
        json!({ "name": "EVENT_INGEST_SECRET", "valueFrom": { "secretKeyRef": { "name": "dd-agent-secrets", "key": "SERVER_AUTH_SECRET" } } }),
        json!({ "name": "NATS_URL", "value": "nats://dd-nats.messaging.svc.cluster.local:4222" }),
        json!({ "name": "NATS_EVENT_SUBJECT", "value": RUNTIME_EVENTS_SUBJECT }),
    ];
    if let Some(thread_title) = thread_title
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        env.push(json!({ "name": "REMOTE_DEV_THREAD_TITLE", "value": thread_title }));
    }
    json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/part-of": "dd-remote-dev",
                "app.kubernetes.io/component": "thread-pod",
                "dd/threadId": thread_id
            }
        },
        "spec": {
            "replicas": 1,
            "strategy": { "type": "Recreate" },
            "selector": { "matchLabels": { "dd/threadId": thread_id } },
            "template": {
                "metadata": {
                    "labels": {
                        "app.kubernetes.io/part-of": "dd-remote-dev",
                        "app.kubernetes.io/component": "thread-pod",
                        "dd/threadId": thread_id
                    }
                },
                "spec": {
                    "terminationGracePeriodSeconds": 30,
                    "initContainers": [{
                        "name": "workspace-permissions",
                        "image": "docker.io/library/busybox:1.36",
                        "imagePullPolicy": "IfNotPresent",
                        "command": ["/bin/sh", "-c"],
                        "args": ["mkdir -p /home/node/workspace /tmp/convos && chown -R 1000:1000 /home/node/workspace /tmp/convos"],
                        "volumeMounts": [
                            { "name": "workspace", "mountPath": "/home/node/workspace" },
                            { "name": "tmp-convos", "mountPath": "/tmp/convos" }
                        ]
                    }],
                    "containers": [{
                        "name": "dev-server",
                        "image": image,
                        "imagePullPolicy": "IfNotPresent",
                        "securityContext": {
                            "runAsNonRoot": true,
                            "runAsUser": 1000,
                            "runAsGroup": 1000
                        },
                        "ports": [{ "containerPort": 8080, "name": "http" }],
                        "env": env,
                        "envFrom": [
                            { "secretRef": { "name": "dd-agent-secrets", "optional": true } }
                        ],
                        "volumeMounts": [
                            { "name": "workspace", "mountPath": "/home/node/workspace" },
                            { "name": "tmp-convos", "mountPath": "/tmp/convos" }
                        ],
                        "resources": {
                            "requests": { "cpu": "1m", "memory": "512Mi" },
                            "limits": { "cpu": "2", "memory": "4Gi" }
                        },
                        "startupProbe": {
                            "httpGet": { "path": "/healthz", "port": "http" },
                            "periodSeconds": 5,
                            "failureThreshold": 180
                        },
                        "livenessProbe": {
                            "httpGet": { "path": "/healthz", "port": "http" },
                            "periodSeconds": 30,
                            "timeoutSeconds": 5,
                            "failureThreshold": 3
                        },
                        "readinessProbe": {
                            "httpGet": { "path": "/healthz", "port": "http" },
                            "periodSeconds": 10,
                            "timeoutSeconds": 3,
                            "failureThreshold": 2
                        }
                    }],
                    "volumes": [
                        {
                            "name": "workspace",
                            "hostPath": {
                                "path": format!("/home/ec2-user/codes/dd/thread-workspaces/{name}"),
                                "type": "DirectoryOrCreate"
                            }
                        },
                        {
                            "name": "tmp-convos",
                            "emptyDir": { "sizeLimit": "256Mi" }
                        }
                    ]
                }
            }
        }
    })
}

pub(crate) async fn ensure_thread_worker(
    thread_id: &str,
    repo_url: &str,
    base_branch: &str,
    thread_title: Option<&str>,
) -> Result<(String, String, Vec<ThreadActionResult>), String> {
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(thread_id);
    let mut results = Vec::new();
    if let Err(error) = prune_awake_thread_workers_for_capacity(&namespace, &name).await {
        tracing::error!("thread capacity prune skipped before waking {name}: {error}");
    }
    let deployment = render_thread_deployment(
        &namespace,
        &name,
        thread_id,
        repo_url,
        base_branch,
        thread_title,
    );

    results.push(
        k8s_create_request(
            format!("/api/v1/namespaces/{namespace}/services"),
            render_thread_service(&namespace, &name, thread_id),
        )
        .await?,
    );
    results.push(
        k8s_create_request(
            format!("/apis/apps/v1/namespaces/{namespace}/deployments"),
            deployment.clone(),
        )
        .await?,
    );
    results.push(
        k8s_json_request(
            reqwest::Method::PATCH,
            format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}"),
            Some(json!({ "spec": deployment["spec"].clone() })),
            "application/merge-patch+json",
        )
        .await?,
    );
    results.push(
        k8s_json_request(
            reqwest::Method::PATCH,
            format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale"),
            Some(json!({ "spec": { "replicas": 1 } })),
            "application/merge-patch+json",
        )
        .await?,
    );

    Ok((namespace, name, results))
}

pub(crate) async fn prepare_thread_worker(thread_id: &str) -> Result<ThreadActionResponse, String> {
    let repo_config = fetch_thread_repo_config_from_postgres(thread_id)
        .await?
        .ok_or_else(|| "thread repo config is not configured".to_string())?;
    let (namespace, name, results) = ensure_thread_worker(
        thread_id,
        &repo_config.repo,
        &repo_config.base_branch,
        repo_config.thread_title.as_deref(),
    )
    .await?;
    let Some(secret) = worker_auth_secret() else {
        return Err(missing_worker_auth_secret_message().to_string());
    };
    wait_thread_worker_ready(&namespace, &name, &secret).await?;

    Ok(ThreadActionResponse {
        ok: true,
        action: "prepare".to_string(),
        thread_id: thread_id.to_string(),
        k8s_name: name,
        namespace,
        results,
        errors: Vec::new(),
    })
}

pub(crate) async fn scale_thread_runtime(
    thread_id: String,
    action: &'static str,
    replicas: i32,
    task_id: Option<String>,
) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/scale",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let path = format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale");
    let mut response = ThreadActionResponse {
        ok: false,
        action: action.to_string(),
        thread_id,
        k8s_name: name,
        namespace,
        results: Vec::new(),
        errors: Vec::new(),
    };
    match k8s_json_request(
        reqwest::Method::PATCH,
        path,
        Some(json!({ "spec": { "replicas": replicas } })),
        "application/merge-patch+json",
    )
    .await
    {
        Ok(result) => {
            response.ok = result.ok;
            response.results.push(result);
        }
        Err(error) => response.errors.push(error),
    }
    if response.ok {
        let status = match action {
            "sleep" => "sleeping",
            "archive" => "archived",
            _ if replicas == 0 => "suspended",
            _ => "awake",
        };
        if let Err(error) = publish_thread_runtime_event_to_nats(
            &response.thread_id,
            task_id.as_deref(),
            action,
            status,
            "thread runtime scaled",
        )
        .await
        {
            tracing::error!("failed to publish thread runtime event: {error}");
        }
    }
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    (status, Json(response)).into_response()
}

pub(crate) async fn delete_thread_runtime(thread_id: String, task_id: Option<String>) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/hard-delete",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let resources = [
        format!("/apis/networking.k8s.io/v1/namespaces/{namespace}/ingresses/{name}"),
        format!("/api/v1/namespaces/{namespace}/services/{name}"),
        format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}"),
        format!("/api/v1/namespaces/{namespace}/persistentvolumeclaims/{name}"),
    ];
    let mut response = ThreadActionResponse {
        ok: false,
        action: "hard-delete".to_string(),
        thread_id,
        k8s_name: name,
        namespace,
        results: Vec::new(),
        errors: Vec::new(),
    };
    for path in resources {
        match k8s_json_request(reqwest::Method::DELETE, path, None, "application/json").await {
            Ok(result) => response.results.push(result),
            Err(error) => response.errors.push(error),
        }
    }
    response.ok = response.errors.is_empty() && response.results.iter().all(|result| result.ok);
    if response.ok {
        if let Err(error) = publish_thread_runtime_event_to_nats(
            &response.thread_id,
            task_id.as_deref(),
            "hard-delete",
            "deleted",
            "thread runtime resources deleted",
        )
        .await
        {
            tracing::error!("failed to publish thread runtime event: {error}");
        }
    }
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    (status, Json(response)).into_response()
}

pub(crate) async fn wait_thread_worker_ready(namespace: &str, name: &str, secret: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|error| format!("failed to build worker readiness client: {error}"))?;
    let url = thread_worker_url(namespace, name, "/healthz");
    for _ in 0..100 {
        if let Ok(response) = client
            .get(&url)
            .header("X-Server-Auth", secret)
            .send()
            .await
        {
            if response.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    Err("thread worker readiness timed out".to_string())
}

pub(crate) async fn ensure_thread_worker_for_control(
    thread_id: &str,
    action: &'static str,
    task_id: Option<&str>,
    waking_message: &'static str,
    awake_message: &'static str,
) -> Result<(String, String, String), ThreadActionResponse> {
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(thread_id);
    let Some(secret) = worker_auth_secret() else {
        return Err(ThreadActionResponse {
            ok: false,
            action: action.to_string(),
            thread_id: thread_id.to_string(),
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![missing_worker_auth_secret_message().to_string()],
        });
    };

    if let Err(error) =
        publish_thread_runtime_event_to_nats(thread_id, task_id, action, "waking", waking_message)
            .await
    {
        tracing::error!("failed to publish thread runtime event: {error}");
    }

    let repo_config = match fetch_thread_repo_config_from_postgres(thread_id).await {
        Ok(Some(repo_config)) => repo_config,
        Ok(None) => {
            return Err(ThreadActionResponse {
                ok: false,
                action: action.to_string(),
                thread_id: thread_id.to_string(),
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec!["thread repo config is not configured".to_string()],
            });
        }
        Err(error) => {
            return Err(ThreadActionResponse {
                ok: false,
                action: action.to_string(),
                thread_id: thread_id.to_string(),
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec![error],
            });
        }
    };

    let (namespace, name, _results) = match ensure_thread_worker(
        thread_id,
        &repo_config.repo,
        &repo_config.base_branch,
        repo_config.thread_title.as_deref(),
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            return Err(ThreadActionResponse {
                ok: false,
                action: action.to_string(),
                thread_id: thread_id.to_string(),
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec![error],
            });
        }
    };

    if let Err(error) = wait_thread_worker_ready(&namespace, &name, &secret).await {
        return Err(ThreadActionResponse {
            ok: false,
            action: action.to_string(),
            thread_id: thread_id.to_string(),
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        });
    }

    if let Err(error) =
        publish_thread_runtime_event_to_nats(thread_id, task_id, action, "awake", awake_message)
            .await
    {
        tracing::error!("failed to publish thread runtime event: {error}");
    }

    Ok((namespace, name, secret))
}

pub(crate) async fn merge_thread_upstream(thread_id: String, request: ThreadControlRequest) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/merge-upstream",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let Some(secret) = worker_auth_secret() else {
        let response = ThreadActionResponse {
            ok: false,
            action: "merge-upstream".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![missing_worker_auth_secret_message().to_string()],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    };

    let scale_path = format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale");
    if let Err(error) = publish_thread_runtime_event_to_nats(
        &thread_id,
        request.task_id.as_deref(),
        "merge-upstream",
        "waking",
        "waking thread runtime for merge",
    )
    .await
    {
        tracing::error!("failed to publish thread runtime event: {error}");
    }
    if let Err(error) = k8s_json_request(
        reqwest::Method::PATCH,
        scale_path,
        Some(json!({ "spec": { "replicas": 1 } })),
        "application/merge-patch+json",
    )
    .await
    {
        let response = ThreadActionResponse {
            ok: false,
            action: "merge-upstream".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    }

    if let Err(error) = wait_thread_worker_ready(&namespace, &name, &secret).await {
        let response = ThreadActionResponse {
            ok: false,
            action: "merge-upstream".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    }
    if let Err(error) = publish_thread_runtime_event_to_nats(
        &thread_id,
        request.task_id.as_deref(),
        "merge-upstream",
        "awake",
        "thread runtime ready for merge",
    )
    .await
    {
        tracing::error!("failed to publish thread runtime event: {error}");
    }

    let client = reqwest::Client::new();
    let worker_response = client
        .post(thread_worker_url(
            &namespace,
            &name,
            "/thread/merge-upstream",
        ))
        .header("X-Server-Auth", secret)
        .json(&json!({
            "kind": "thread-control",
            "action": "merge-upstream",
            "threadId": thread_id.clone(),
            "taskId": request.task_id,
            "requestedBy": request.requested_by,
            "reason": request.reason,
        }))
        .send()
        .await;
    match worker_response {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let public_status =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (
                public_status,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(error) => {
            let response = ThreadActionResponse {
                ok: false,
                action: "merge-upstream".to_string(),
                thread_id,
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec![error.to_string()],
            };
            (StatusCode::BAD_GATEWAY, Json(response)).into_response()
        }
    }
}

pub(crate) async fn open_thread_pr(thread_id: String, request: ThreadControlRequest) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/open-pr",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let Some(secret) = worker_auth_secret() else {
        let response = ThreadActionResponse {
            ok: false,
            action: "open-pr".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![missing_worker_auth_secret_message().to_string()],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    };

    let scale_path = format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale");
    if let Err(error) = publish_thread_runtime_event_to_nats(
        &thread_id,
        request.task_id.as_deref(),
        "open-pr",
        "waking",
        "waking thread runtime for draft PR",
    )
    .await
    {
        tracing::error!("failed to publish thread runtime event: {error}");
    }
    if let Err(error) = k8s_json_request(
        reqwest::Method::PATCH,
        scale_path,
        Some(json!({ "spec": { "replicas": 1 } })),
        "application/merge-patch+json",
    )
    .await
    {
        let response = ThreadActionResponse {
            ok: false,
            action: "open-pr".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    }
    if let Err(error) = wait_thread_worker_ready(&namespace, &name, &secret).await {
        let response = ThreadActionResponse {
            ok: false,
            action: "open-pr".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    }
    if let Err(error) = publish_thread_runtime_event_to_nats(
        &thread_id,
        request.task_id.as_deref(),
        "open-pr",
        "awake",
        "thread runtime ready for draft PR",
    )
    .await
    {
        tracing::error!("failed to publish thread runtime event: {error}");
    }

    let client = reqwest::Client::new();
    let worker_response = client
        .post(thread_worker_url(&namespace, &name, "/thread/open-pr"))
        .header("X-Server-Auth", secret)
        .json(&json!({
            "kind": "thread-control",
            "action": "open-pr",
            "threadId": thread_id.clone(),
            "taskId": request.task_id,
            "requestedBy": request.requested_by,
            "reason": request.reason,
        }))
        .send()
        .await;
    match worker_response {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let public_status =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (
                public_status,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(error) => {
            let response = ThreadActionResponse {
                ok: false,
                action: "open-pr".to_string(),
                thread_id,
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec![error.to_string()],
            };
            (StatusCode::BAD_GATEWAY, Json(response)).into_response()
        }
    }
}

pub(crate) async fn make_thread_commit(thread_id: String, request: ThreadControlRequest) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/make-commit",
        StatusCode::OK,
    );
    let (namespace, name, secret) = match ensure_thread_worker_for_control(
        &thread_id,
        "make-commit",
        request.task_id.as_deref(),
        "waking thread runtime for commit",
        "thread runtime ready for commit",
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return (StatusCode::BAD_GATEWAY, Json(response)).into_response(),
    };

    let client = reqwest::Client::new();
    let worker_response = client
        .post(thread_worker_url(&namespace, &name, "/thread/make-commit"))
        .header("X-Server-Auth", secret)
        .json(&json!({
            "kind": "thread-control",
            "action": "make-commit",
            "threadId": thread_id.clone(),
            "taskId": request.task_id,
            "requestedBy": request.requested_by,
            "reason": request.reason,
        }))
        .send()
        .await;
    match worker_response {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let public_status =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (
                public_status,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(error) => {
            let response = ThreadActionResponse {
                ok: false,
                action: "make-commit".to_string(),
                thread_id,
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec![error.to_string()],
            };
            (StatusCode::BAD_GATEWAY, Json(response)).into_response()
        }
    }
}

pub(crate) async fn open_thread_terminal(thread_id: String, request: ThreadControlRequest) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/terminal",
        StatusCode::OK,
    );
    let (namespace, name, _secret) = match ensure_thread_worker_for_control(
        &thread_id,
        "terminal",
        request.task_id.as_deref(),
        "waking thread runtime for terminal",
        "thread runtime ready for terminal",
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return (StatusCode::BAD_GATEWAY, Json(response)).into_response(),
    };

    let terminal_url = thread_terminal_url(&thread_id);
    Json(json!({
        "ok": true,
        "action": "terminal",
        "threadId": thread_id,
        "k8sName": name,
        "namespace": namespace,
        "terminalUrl": terminal_url,
    }))
    .into_response()
}

pub(crate) async fn prepare_thread(headers: HeaderMap, Path(thread_id): Path<String>) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/prepare",
        StatusCode::OK,
    );
    if !authorized_internal_request(&headers) {
        return unauthorized_response();
    }

    match prepare_thread_worker(&thread_id).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!("thread worker prepare failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": public_thread_worker_proxy_error("prepare") })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn thread_runtime(Path(thread_id): Path<String>) -> Response {
    record_request(
        "GET",
        "/api/agents/threads/:threadId/runtime",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let mut errors = Vec::new();

    let deployment = match k8s_get_value(format!(
        "/apis/apps/v1/namespaces/{namespace}/deployments/{name}"
    ))
    .await
    {
        Ok(Some(value)) => Some(summarize_deployment(&value)),
        Ok(None) => None,
        Err(error) => {
            errors.push(error);
            None
        }
    };
    let service =
        match k8s_get_value(format!("/api/v1/namespaces/{namespace}/services/{name}")).await {
            Ok(Some(value)) => Some(summarize_service(&value)),
            Ok(None) => None,
            Err(error) => {
                errors.push(error);
                None
            }
        };
    let pods = match k8s_get_value(format!(
        "/api/v1/namespaces/{namespace}/pods?labelSelector=dd%2FthreadId%3D{thread_id}"
    ))
    .await
    {
        Ok(Some(value)) => json_at(&value, &["items"])
            .and_then(Value::as_array)
            .map(|items| items.iter().map(summarize_pod).collect::<Vec<_>>())
            .unwrap_or_default(),
        Ok(None) => Vec::new(),
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    let summary = summarize_thread_runtime(deployment.as_ref(), &pods);
    Json(ThreadRuntimeResponse {
        ok: errors.is_empty(),
        source: "kubernetes".to_string(),
        thread_id,
        namespace,
        k8s_name: name,
        generated_at_ms: now_ms(),
        summary,
        deployment,
        service,
        pods,
        errors,
    })
    .into_response()
}

pub(crate) async fn stream_thread_task(Path((thread_id, task_id)): Path<(String, String)>) -> Response {
    record_request(
        "GET",
        "/api/agents/threads/:threadId/stream/:taskId",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let Some(secret) = worker_auth_secret() else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": missing_worker_auth_secret_message() })),
        )
            .into_response();
    };
    let response = reqwest::Client::new()
        .get(thread_worker_url(
            &namespace,
            &name,
            &format!("/stream/{task_id}"),
        ))
        .header("X-Server-Auth", secret)
        .send()
        .await;
    match response {
        Ok(response) => {
            let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::OK);
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from_stream(response.bytes_stream()))
                .unwrap_or_else(|error| {
                    (
                        StatusCode::BAD_GATEWAY,
                        format!("stream response build failed: {error}"),
                    )
                        .into_response()
                })
        }
        Err(error) => {
            tracing::error!("thread worker stream proxy failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                public_thread_worker_proxy_error("stream"),
            )
                .into_response()
        }
    }
}

pub(crate) async fn sleep_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "sleep", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    scale_thread_runtime(thread_id, "sleep", 0, request.task_id.clone()).await
}

pub(crate) async fn archive_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "archive", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    scale_thread_runtime(thread_id, "archive", 0, request.task_id.clone()).await
}

pub(crate) async fn hard_delete_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "hard-delete", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    delete_thread_runtime(thread_id, request.task_id.clone()).await
}

pub(crate) async fn merge_upstream_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "merge-upstream", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    merge_thread_upstream(thread_id, request).await
}

pub(crate) async fn open_pr_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "open-pr", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    open_thread_pr(thread_id, request).await
}

pub(crate) async fn make_commit_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "make-commit", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    make_thread_commit(thread_id, request).await
}

pub(crate) async fn terminal_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "terminal", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    open_thread_terminal(thread_id, request).await
}
