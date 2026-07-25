use std::{env, fs};

use serde_json::{json, Value};

use crate::shared::{env_bool, env_usize, json_at, json_at_i64, json_at_string, json_string};
use crate::types::ThreadActionResult;

pub(crate) async fn k8s_http_client() -> Result<(reqwest::Client, String, String), String> {
    let host = env::var("KUBERNETES_SERVICE_HOST")
        .map_err(|_| "KUBERNETES_SERVICE_HOST is not set".to_string())?;
    let port = env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_string());
    let token = fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/token")
        .map_err(|error| format!("failed to read serviceaccount token: {error}"))?;
    let mut builder = reqwest::Client::builder();
    if let Ok(ca) = fs::read("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt") {
        if let Ok(cert) = reqwest::Certificate::from_pem(&ca) {
            builder = builder.add_root_certificate(cert);
        }
    }
    let client = builder
        .build()
        .map_err(|error| format!("failed to build k8s http client: {error}"))?;
    Ok((client, format!("https://{host}:{port}"), token))
}

pub(crate) async fn k8s_json_request(
    method: reqwest::Method,
    path: String,
    body: Option<Value>,
    content_type: &'static str,
) -> Result<ThreadActionResult, String> {
    let (client, base_url, token) = k8s_http_client().await?;
    let mut request = client
        .request(method, format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(body) = body {
        request = request
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("k8s request failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok(ThreadActionResult {
        resource: path,
        status: status.as_u16(),
        ok: status.is_success() || status == reqwest::StatusCode::NOT_FOUND,
        body: body.chars().take(500).collect(),
    })
}

pub(crate) async fn k8s_create_request(path: String, body: Value) -> Result<ThreadActionResult, String> {
    let (client, base_url, token) = k8s_http_client().await?;
    let response = client
        .post(format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("k8s create failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok(ThreadActionResult {
        resource: path,
        status: status.as_u16(),
        ok: status.is_success() || status == reqwest::StatusCode::CONFLICT,
        body: body.chars().take(500).collect(),
    })
}

pub(crate) async fn k8s_get_value(path: String) -> Result<Option<Value>, String> {
    let (client, base_url, token) = k8s_http_client().await?;
    let response = client
        .get(format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| format!("k8s get failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!(
            "k8s get {path} failed {}: {}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        ));
    }
    serde_json::from_str::<Value>(&body)
        .map(Some)
        .map_err(|error| format!("k8s get {path} returned invalid json: {error}"))
}

pub(crate) fn summarize_deployment(deployment: &Value) -> Value {
    let conditions = json_at(deployment, &["status", "conditions"])
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|condition| {
                    json!({
                        "type": json_string(condition, "type"),
                        "status": json_string(condition, "status"),
                        "reason": json_string(condition, "reason"),
                        "message": json_string(condition, "message"),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "name": json_at_string(deployment, &["metadata", "name"]),
        "createdAt": json_at_string(deployment, &["metadata", "creationTimestamp"]),
        "desiredReplicas": json_at_i64(deployment, &["spec", "replicas"]).unwrap_or(0),
        "replicas": json_at_i64(deployment, &["status", "replicas"]).unwrap_or(0),
        "readyReplicas": json_at_i64(deployment, &["status", "readyReplicas"]).unwrap_or(0),
        "availableReplicas": json_at_i64(deployment, &["status", "availableReplicas"]).unwrap_or(0),
        "updatedReplicas": json_at_i64(deployment, &["status", "updatedReplicas"]).unwrap_or(0),
        "unavailableReplicas": json_at_i64(deployment, &["status", "unavailableReplicas"]).unwrap_or(0),
        "observedGeneration": json_at_i64(deployment, &["status", "observedGeneration"]),
        "conditions": conditions,
    })
}

pub(crate) fn summarize_service(service: &Value) -> Value {
    json!({
        "name": json_at_string(service, &["metadata", "name"]),
        "createdAt": json_at_string(service, &["metadata", "creationTimestamp"]),
        "clusterIp": json_at_string(service, &["spec", "clusterIP"]),
        "type": json_at_string(service, &["spec", "type"]),
    })
}

pub(crate) fn summarize_pod(pod: &Value) -> Value {
    let conditions = json_at(pod, &["status", "conditions"])
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|condition| {
                    json!({
                        "type": json_string(condition, "type"),
                        "status": json_string(condition, "status"),
                        "reason": json_string(condition, "reason"),
                        "message": json_string(condition, "message"),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let init_containers = json_at(pod, &["status", "initContainerStatuses"])
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|container| {
                    json!({
                        "name": json_string(container, "name"),
                        "ready": container.get("ready").and_then(Value::as_bool).unwrap_or(false),
                        "restartCount": json_at_i64(container, &["restartCount"]).unwrap_or(0),
                        "state": container.get("state").cloned().unwrap_or_else(|| json!({})),
                        "lastState": container.get("lastState").cloned().unwrap_or_else(|| json!({})),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let container_specs = json_at(pod, &["spec", "containers"])
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|container| {
                    json!({
                        "name": json_string(container, "name"),
                        "resources": container.get("resources").cloned().unwrap_or_else(|| json!({})),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let containers = json_at(pod, &["status", "containerStatuses"])
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|container| {
                    json!({
                        "name": json_string(container, "name"),
                        "ready": container.get("ready").and_then(Value::as_bool).unwrap_or(false),
                        "restartCount": json_at_i64(container, &["restartCount"]).unwrap_or(0),
                        "state": container.get("state").cloned().unwrap_or_else(|| json!({})),
                        "lastState": container.get("lastState").cloned().unwrap_or_else(|| json!({})),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "name": json_at_string(pod, &["metadata", "name"]),
        "createdAt": json_at_string(pod, &["metadata", "creationTimestamp"]),
        "phase": json_at_string(pod, &["status", "phase"]),
        "podIp": json_at_string(pod, &["status", "podIP"]),
        "hostIp": json_at_string(pod, &["status", "hostIP"]),
        "startTime": json_at_string(pod, &["status", "startTime"]),
        "deletionTimestamp": json_at_string(pod, &["metadata", "deletionTimestamp"]),
        "conditions": conditions,
        "initContainers": init_containers,
        "containerSpecs": container_specs,
        "containers": containers,
    })
}

pub(crate) fn summarize_thread_runtime(deployment: Option<&Value>, pods: &[Value]) -> Value {
    let desired = deployment
        .and_then(|value| json_at_i64(value, &["desiredReplicas"]))
        .unwrap_or(0);
    let available = deployment
        .and_then(|value| json_at_i64(value, &["availableReplicas"]))
        .unwrap_or(0);
    let ready = deployment
        .and_then(|value| json_at_i64(value, &["readyReplicas"]))
        .unwrap_or(0);
    let ready_pods = pods
        .iter()
        .filter(|pod| {
            json_at(pod, &["containers"])
                .and_then(Value::as_array)
                .is_some_and(|containers| {
                    !containers.is_empty()
                        && containers.iter().all(|container| {
                            container.get("ready").and_then(Value::as_bool) == Some(true)
                        })
                })
        })
        .count();
    let phase = if deployment.is_none() {
        "missing"
    } else if desired == 0 {
        "sleeping"
    } else if available > 0 && ready > 0 {
        "ready"
    } else if pods.is_empty() {
        "creating"
    } else {
        "starting"
    };
    json!({
        "phase": phase,
        "desiredReplicas": desired,
        "readyReplicas": ready,
        "availableReplicas": available,
        "podCount": pods.len(),
        "readyPodCount": ready_pods,
    })
}

pub(crate) async fn prune_awake_thread_workers_for_capacity(
    namespace: &str,
    current_name: &str,
) -> Result<Vec<String>, String> {
    if !env_bool("THREAD_RUNTIME_CAPACITY_PRUNE_ENABLED", true) {
        return Ok(Vec::new());
    }
    let max_awake = env_usize("THREAD_RUNTIME_MAX_AWAKE_DEPLOYMENTS", 4);
    let value = match k8s_get_value(format!(
        "/apis/apps/v1/namespaces/{namespace}/deployments?labelSelector=app.kubernetes.io%2Fcomponent%3Dthread-pod"
    ))
    .await?
    {
        Some(value) => value,
        None => return Ok(Vec::new()),
    };
    let mut awake = json_at(&value, &["items"])
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|deployment| {
                    let name = json_at_string(deployment, &["metadata", "name"])?;
                    let desired = json_at_i64(deployment, &["spec", "replicas"]).unwrap_or(0);
                    if desired <= 0 || name == current_name {
                        return None;
                    }
                    let created_at = json_at_string(deployment, &["metadata", "creationTimestamp"])
                        .unwrap_or_default();
                    Some((created_at, name))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let current_awake_slots = 1usize;
    if awake.len() + current_awake_slots <= max_awake {
        return Ok(Vec::new());
    }
    awake.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    let keep_other_awake = max_awake.saturating_sub(current_awake_slots);
    let mut slept = Vec::new();
    for (_created_at, name) in awake.into_iter().skip(keep_other_awake) {
        let path = format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale");
        match k8s_json_request(
            reqwest::Method::PATCH,
            path,
            Some(json!({ "spec": { "replicas": 0 } })),
            "application/merge-patch+json",
        )
        .await
        {
            Ok(result) if result.ok => slept.push(name),
            Ok(result) => tracing::error!(
                "thread capacity prune scale failed: {} status={} body={}",
                result.resource,
                result.status,
                result.body
            ),
            Err(error) => tracing::error!("thread capacity prune failed for {name}: {error}"),
        }
    }
    if !slept.is_empty() {
        tracing::error!(
            "thread capacity prune slept {} old workers before waking {current_name}: {}",
            slept.len(),
            slept.join(", ")
        );
    }
    Ok(slept)
}
