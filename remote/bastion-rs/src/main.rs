use std::{
    env,
    net::SocketAddr,
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    signal,
};

const SERVICE_NAME: &str = "dd-bastion";
const DEFAULT_PORT: u16 = 8111;
const DEFAULT_CA_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";
const DEFAULT_TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";
const TERMINAL_SHELL: &str =
    "if command -v bash >/dev/null 2>&1; then exec bash -i; else exec sh -i; fi";

#[derive(Clone, Copy)]
struct ManagedDeployment {
    slug: &'static str,
    title: &'static str,
    namespace: &'static str,
    deployment: &'static str,
    service: &'static str,
    access: &'static str,
    notes: &'static str,
}

const MANAGED_DEPLOYMENTS: &[ManagedDeployment] = &[
    ManagedDeployment {
        slug: "solana-contracts",
        title: "Solana contract server",
        namespace: "default",
        deployment: "dd-contract-service",
        service: "dd-contract-service.default.svc.cluster.local:8101",
        access: "server auth",
        notes: "Rust Solana contract validation and simulation gateway.",
    },
    ManagedDeployment {
        slug: "vpn",
        title: "VPN server",
        namespace: "vpn",
        deployment: "dd-vpn",
        service: "dd-vpn-ui.vpn.svc.cluster.local:51821",
        access: "vpn/private",
        notes: "WireGuard wg-easy overlay and private admin UI.",
    },
    ManagedDeployment {
        slug: "live-mutex",
        title: "Live mutex server",
        namespace: "default",
        deployment: "dd-live-mutex",
        service: "dd-live-mutex.default.svc.cluster.local:6970",
        access: "cluster local",
        notes: "Singleton TCP lock broker for live-mutex clients.",
    },
    ManagedDeployment {
        slug: "bastion",
        title: "Bastion/jumphost server",
        namespace: "vpn",
        deployment: "dd-bastion",
        service: "dd-bastion.vpn.svc.cluster.local:8111",
        access: "server auth",
        notes: "Authenticated access broker for VPN, kubeconfig, runtime inventory, and exec terminals.",
    },
    ManagedDeployment {
        slug: "redis-cache",
        title: "Redis cache server",
        namespace: "default",
        deployment: "dd-redis-cache",
        service: "dd-redis-cache.default.svc.cluster.local:6379",
        access: "cluster local",
        notes: "Ephemeral Redis cache with memory cap and health probes.",
    },
    ManagedDeployment {
        slug: "live-mutex-loadtest",
        title: "Node.js live-mutex load test",
        namespace: "default",
        deployment: "dd-live-mutex-loadtest-node",
        service: "dd-live-mutex-loadtest-node.default.svc.cluster.local",
        access: "internal",
        notes: "Node.js aggregate load generator for the live-mutex broker.",
    },
    ManagedDeployment {
        slug: "trading",
        title: "Rust algorithmic trading server",
        namespace: "default",
        deployment: "dd-trading-server",
        service: "dd-trading-server.default.svc.cluster.local:8103",
        access: "server auth",
        notes: "Risk-gated trading decision service for paper/live order intents.",
    },
    ManagedDeployment {
        slug: "container-pool",
        title: "Container pool service",
        namespace: "default",
        deployment: "dd-container-pool",
        service: "dd-container-pool.default.svc.cluster.local:8102",
        access: "server auth",
        notes: "Rust warm-worker pool backed by Postgres config and local containerd.",
    },
];

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
}

#[derive(Clone)]
struct Config {
    server_auth_secret: Option<String>,
    public_base_url: String,
    wireguard_endpoint: String,
    vpn_cidr: String,
    service_cidr: String,
    pod_cidr: String,
    dns: String,
    kube_api_server: String,
    kube_cluster_name: String,
    kube_context_name: String,
    kube_user_name: String,
    ca_path: String,
    token_path: String,
    kubectl_bin: String,
    script_bin: String,
    kubeconfig_enabled: bool,
    include_serviceaccount_token: bool,
    terminal_enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    auth_configured: bool,
    kubeconfig_enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccessProfile {
    service: &'static str,
    auth_required: bool,
    vpn: VpnProfile,
    cluster: ClusterProfile,
    endpoints: EndpointProfile,
    examples: Vec<String>,
    notes: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VpnProfile {
    wireguard_endpoint: String,
    vpn_cidr: String,
    dns: String,
    split_tunnel_allowed_ips: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClusterProfile {
    api_server: String,
    service_cidr: String,
    pod_cidr: String,
    kubeconfig_endpoint: String,
    kubeconfig_mode: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EndpointProfile {
    bastion_url: String,
    healthz: String,
    profile: String,
    kubeconfig: String,
    runtime_deployments: String,
    terminal: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedDeploymentInfo {
    slug: &'static str,
    title: &'static str,
    namespace: &'static str,
    deployment: &'static str,
    service: &'static str,
    access: &'static str,
    notes: &'static str,
    summary: Value,
    pods: Vec<Value>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeDeploymentsResponse {
    ok: bool,
    service: &'static str,
    generated_at_ms: u128,
    terminal_enabled: bool,
    deployments: Vec<ManagedDeploymentInfo>,
    errors: Vec<String>,
}

#[derive(Clone)]
struct TerminalTarget {
    namespace: String,
    deployment: String,
    pod: String,
    container: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalQuery {
    namespace: String,
    deployment: String,
    pod: String,
    container: String,
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(fallback)
}

fn config_from_env() -> Config {
    Config {
        server_auth_secret: first_env(&["BASTION_AUTH_SECRET", "SERVER_AUTH_SECRET"]),
        public_base_url: env_value(
            "BASTION_PUBLIC_BASE_URL",
            "http://dd-bastion.vpn.svc.cluster.local:8111",
        ),
        wireguard_endpoint: env_value("BASTION_WIREGUARD_ENDPOINT", "54.91.17.58:51820"),
        vpn_cidr: env_value("BASTION_VPN_CIDR", "10.8.0.0/24"),
        service_cidr: env_value("BASTION_SERVICE_CIDR", "10.96.0.0/12"),
        pod_cidr: env_value("BASTION_POD_CIDR", "10.244.0.0/16"),
        dns: env_value("BASTION_DNS", "10.96.0.10"),
        kube_api_server: env_value("BASTION_KUBE_API_SERVER", "https://kubernetes.default.svc"),
        kube_cluster_name: env_value("BASTION_KUBE_CLUSTER_NAME", "dd-remote-dev"),
        kube_context_name: env_value("BASTION_KUBE_CONTEXT_NAME", "dd-vpn-access-broker"),
        kube_user_name: env_value("BASTION_KUBE_USER_NAME", "dd-bastion-access-broker"),
        ca_path: env_value("BASTION_KUBE_CA_PATH", DEFAULT_CA_PATH),
        token_path: env_value("BASTION_KUBE_TOKEN_PATH", DEFAULT_TOKEN_PATH),
        kubectl_bin: env_value("BASTION_KUBECTL_BIN", "/usr/bin/kubectl"),
        script_bin: env_value("BASTION_SCRIPT_BIN", "/usr/bin/script"),
        kubeconfig_enabled: env_bool("BASTION_KUBECONFIG_ENABLED", true),
        include_serviceaccount_token: env_bool("BASTION_INCLUDE_SERVICEACCOUNT_TOKEN", true),
        terminal_enabled: env_bool("BASTION_TERMINAL_ENABLED", true),
    }
}

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    let direct_header_matches = headers
        .get("x-bastion-auth")
        .or_else(|| headers.get("x-server-auth"))
        .or_else(|| headers.get("auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == secret);

    let bearer_matches = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value == secret);

    direct_header_matches || bearer_matches
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "SERVER_AUTH_SECRET is not configured" })),
        )
            .into_response());
    };

    if !request_is_authorized(headers, secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "errMessage": "missing required bastion auth header",
            })),
        )
            .into_response());
    }

    Ok(())
}

fn split_allowed_ips(config: &Config) -> Vec<String> {
    vec![
        config.vpn_cidr.clone(),
        config.service_cidr.clone(),
        config.pod_cidr.clone(),
    ]
}

fn access_profile(config: &Config) -> AccessProfile {
    let base = config.public_base_url.trim_end_matches('/').to_string();
    AccessProfile {
        service: SERVICE_NAME,
        auth_required: true,
        vpn: VpnProfile {
            wireguard_endpoint: config.wireguard_endpoint.clone(),
            vpn_cidr: config.vpn_cidr.clone(),
            dns: config.dns.clone(),
            split_tunnel_allowed_ips: split_allowed_ips(config),
        },
        cluster: ClusterProfile {
            api_server: config.kube_api_server.clone(),
            service_cidr: config.service_cidr.clone(),
            pod_cidr: config.pod_cidr.clone(),
            kubeconfig_endpoint: format!("{base}/kubeconfig"),
            kubeconfig_mode: if config.include_serviceaccount_token {
                "access-broker service account token".to_string()
            } else {
                "template without token".to_string()
            },
        },
        endpoints: EndpointProfile {
            bastion_url: base.clone(),
            healthz: format!("{base}/healthz"),
            profile: format!("{base}/profile"),
            kubeconfig: format!("{base}/kubeconfig"),
            runtime_deployments: format!("{base}/runtime/deployments"),
            terminal: format!("{base}/terminal"),
        },
        examples: vec![
            format!("curl -H 'X-Bastion-Auth: $SERVER_AUTH_SECRET' {base}/profile"),
            format!(
                "curl -H 'X-Bastion-Auth: $SERVER_AUTH_SECRET' {base}/kubeconfig > dd-vpn.kubeconfig"
            ),
            format!("curl -H 'X-Bastion-Auth: $SERVER_AUTH_SECRET' {base}/runtime/deployments"),
            "KUBECONFIG=dd-vpn.kubeconfig kubectl get pods -A".to_string(),
        ],
        notes: vec![
            "Connect to WireGuard first; this service is ClusterIP-only inside the vpn namespace."
                .to_string(),
            "The generated kubeconfig is intentionally read-only and does not include Kubernetes Secrets access."
                .to_string(),
            "Exec terminals are restricted to the managed deployment allowlist and require pods/exec RBAC."
                .to_string(),
        ],
    }
}

async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        auth_configured: state.config.server_auth_secret.is_some(),
        kubeconfig_enabled: state.config.kubeconfig_enabled,
    })
}

async fn profile(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AccessProfile>, Response> {
    require_auth(&headers, &state)?;
    Ok(Json(access_profile(&state.config)))
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

async fn kubeconfig(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_auth(&headers, &state)?;

    if !state.config.kubeconfig_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "kubeconfig export is disabled" })),
        )
            .into_response());
    }

    let ca_bytes = fs::read(&state.config.ca_path).await.map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "failed to read Kubernetes CA",
                "errMessage": error.to_string(),
            })),
        )
            .into_response()
    })?;
    let ca_data = STANDARD.encode(ca_bytes);

    let token = if state.config.include_serviceaccount_token {
        fs::read_to_string(&state.config.token_path)
            .await
            .map(|value| value.trim().to_string())
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": "failed to read Kubernetes service account token",
                        "errMessage": error.to_string(),
                    })),
                )
                    .into_response()
            })?
    } else {
        "replace-with-a-kubernetes-bearer-token".to_string()
    };

    let body = format!(
        r#"apiVersion: v1
kind: Config
clusters:
  - name: {cluster_name}
    cluster:
      certificate-authority-data: {ca_data}
      server: {api_server}
contexts:
  - name: {context_name}
    context:
      cluster: {cluster_name}
      user: {user_name}
current-context: {context_name}
users:
  - name: {user_name}
    user:
      token: {token}
"#,
        cluster_name = yaml_string(&state.config.kube_cluster_name),
        ca_data = ca_data,
        api_server = yaml_string(&state.config.kube_api_server),
        context_name = yaml_string(&state.config.kube_context_name),
        user_name = yaml_string(&state.config.kube_user_name),
        token = yaml_string(&token),
    );

    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/yaml; charset=utf-8"),
    );
    Ok(response)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn json_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn json_at_string(value: &Value, path: &[&str]) -> Option<String> {
    json_at(value, path)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn json_at_i64(value: &Value, path: &[&str]) -> Option<i64> {
    json_at(value, path).and_then(Value::as_i64)
}

fn selector_from_deployment(deployment: &Value) -> Option<String> {
    let labels = json_at(deployment, &["spec", "selector", "matchLabels"])?.as_object()?;
    let mut parts = labels
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|value| format!("{key}={value}")))
        .collect::<Vec<_>>();
    parts.sort();
    (!parts.is_empty()).then(|| parts.join(","))
}

fn selector_matches_pod(deployment: &Value, pod: &Value) -> bool {
    let Some(selector_labels) =
        json_at(deployment, &["spec", "selector", "matchLabels"]).and_then(Value::as_object)
    else {
        return false;
    };
    let Some(pod_labels) = json_at(pod, &["metadata", "labels"]).and_then(Value::as_object) else {
        return false;
    };
    selector_labels
        .iter()
        .all(|(key, value)| pod_labels.get(key) == Some(value))
}

fn summarize_deployment(deployment: &Value) -> Value {
    let conditions = json_at(deployment, &["status", "conditions"])
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|condition| {
            json!({
                "type": json_at_string(&condition, &["type"]),
                "status": json_at_string(&condition, &["status"]),
                "reason": json_at_string(&condition, &["reason"]),
                "message": json_at_string(&condition, &["message"]),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "name": json_at_string(deployment, &["metadata", "name"]),
        "namespace": json_at_string(deployment, &["metadata", "namespace"]),
        "createdAt": json_at_string(deployment, &["metadata", "creationTimestamp"]),
        "desiredReplicas": json_at_i64(deployment, &["spec", "replicas"]).unwrap_or(0),
        "replicas": json_at_i64(deployment, &["status", "replicas"]).unwrap_or(0),
        "readyReplicas": json_at_i64(deployment, &["status", "readyReplicas"]).unwrap_or(0),
        "availableReplicas": json_at_i64(deployment, &["status", "availableReplicas"]).unwrap_or(0),
        "updatedReplicas": json_at_i64(deployment, &["status", "updatedReplicas"]).unwrap_or(0),
        "unavailableReplicas": json_at_i64(deployment, &["status", "unavailableReplicas"]).unwrap_or(0),
        "conditions": conditions,
    })
}

fn summarize_pod(pod: &Value, deployment_name: &str, terminal_base: &str) -> Value {
    let namespace = json_at_string(pod, &["metadata", "namespace"]).unwrap_or_default();
    let pod_name = json_at_string(pod, &["metadata", "name"]).unwrap_or_default();
    let containers = json_at(pod, &["status", "containerStatuses"])
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|container| {
            let name = json_at_string(&container, &["name"]).unwrap_or_default();
            let terminal_url = if !namespace.is_empty()
                && !deployment_name.is_empty()
                && !pod_name.is_empty()
                && !name.is_empty()
            {
                format!(
                    "{terminal_base}?namespace={namespace}&deployment={deployment_name}&pod={pod_name}&container={name}"
                )
            } else {
                String::new()
            };
            json!({
                "name": name,
                "ready": container.get("ready").and_then(Value::as_bool).unwrap_or(false),
                "restartCount": json_at_i64(&container, &["restartCount"]).unwrap_or(0),
                "image": json_at_string(&container, &["image"]),
                "imageId": json_at_string(&container, &["imageID"]),
                "containerId": json_at_string(&container, &["containerID"]),
                "state": container.get("state").cloned().unwrap_or_else(|| json!({})),
                "lastState": container.get("lastState").cloned().unwrap_or_else(|| json!({})),
                "terminalUrl": terminal_url,
            })
        })
        .collect::<Vec<_>>();

    json!({
        "name": pod_name,
        "namespace": namespace,
        "phase": json_at_string(pod, &["status", "phase"]),
        "nodeName": json_at_string(pod, &["spec", "nodeName"]),
        "podIp": json_at_string(pod, &["status", "podIP"]),
        "createdAt": json_at_string(pod, &["metadata", "creationTimestamp"]),
        "containers": containers,
    })
}

async fn kubectl_json(config: &Config, args: &[String]) -> Result<Value, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new(&config.kubectl_bin).args(args).output(),
    )
    .await
    .map_err(|_| format!("kubectl timed out: {}", args.join(" ")))?
    .map_err(|error| format!("kubectl failed to start: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "kubectl {} failed {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("kubectl {} returned invalid json: {error}", args.join(" ")))
}

async fn executable_available(path: &str) -> bool {
    fs::metadata(path).await.is_ok()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

async fn managed_deployment_info(
    config: &Config,
    target: ManagedDeployment,
    terminal_base: &str,
) -> ManagedDeploymentInfo {
    let mut errors = Vec::new();
    let deployment = match kubectl_json(
        config,
        &[
            "-n".to_string(),
            target.namespace.to_string(),
            "get".to_string(),
            "deployment".to_string(),
            target.deployment.to_string(),
            "-o".to_string(),
            "json".to_string(),
        ],
    )
    .await
    {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(error);
            None
        }
    };

    let selector = deployment.as_ref().and_then(selector_from_deployment);
    let pods = if let Some(selector) = selector {
        match kubectl_json(
            config,
            &[
                "-n".to_string(),
                target.namespace.to_string(),
                "get".to_string(),
                "pods".to_string(),
                "-l".to_string(),
                selector,
                "-o".to_string(),
                "json".to_string(),
            ],
        )
        .await
        {
            Ok(value) => json_at(&value, &["items"])
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|pod| summarize_pod(pod, target.deployment, terminal_base))
                .collect::<Vec<_>>(),
            Err(error) => {
                errors.push(error);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    ManagedDeploymentInfo {
        slug: target.slug,
        title: target.title,
        namespace: target.namespace,
        deployment: target.deployment,
        service: target.service,
        access: target.access,
        notes: target.notes,
        summary: deployment
            .as_ref()
            .map(summarize_deployment)
            .unwrap_or_else(|| json!({})),
        pods,
        errors,
    }
}

async fn runtime_deployments(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RuntimeDeploymentsResponse>, Response> {
    require_auth(&headers, &state)?;
    let terminal_base = "/bastion/terminal";
    let mut deployments = Vec::with_capacity(MANAGED_DEPLOYMENTS.len());
    let mut errors = Vec::new();

    for target in MANAGED_DEPLOYMENTS {
        let info = managed_deployment_info(&state.config, *target, terminal_base).await;
        errors.extend(info.errors.iter().cloned());
        deployments.push(info);
    }

    Ok(Json(RuntimeDeploymentsResponse {
        ok: errors.is_empty(),
        service: SERVICE_NAME,
        generated_at_ms: now_millis(),
        terminal_enabled: state.config.terminal_enabled,
        deployments,
        errors,
    }))
}

fn find_managed_deployment(namespace: &str, deployment: &str) -> Option<ManagedDeployment> {
    MANAGED_DEPLOYMENTS
        .iter()
        .copied()
        .find(|target| target.namespace == namespace && target.deployment == deployment)
}

async fn validate_terminal_target(
    config: &Config,
    query: TerminalQuery,
) -> Result<TerminalTarget, Response> {
    if !config.terminal_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "terminal sessions are disabled" })),
        )
            .into_response());
    }

    if !safe_name(&query.namespace)
        || !safe_name(&query.deployment)
        || !safe_name(&query.pod)
        || !safe_name(&query.container)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid Kubernetes target name" })),
        )
            .into_response());
    }

    let Some(target) = find_managed_deployment(&query.namespace, &query.deployment) else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "deployment is not in the bastion terminal allowlist" })),
        )
            .into_response());
    };

    let deployment = kubectl_json(
        config,
        &[
            "-n".to_string(),
            target.namespace.to_string(),
            "get".to_string(),
            "deployment".to_string(),
            target.deployment.to_string(),
            "-o".to_string(),
            "json".to_string(),
        ],
    )
    .await
    .map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "failed to load deployment", "errMessage": error })),
        )
            .into_response()
    })?;

    let pod = kubectl_json(
        config,
        &[
            "-n".to_string(),
            target.namespace.to_string(),
            "get".to_string(),
            "pod".to_string(),
            query.pod.clone(),
            "-o".to_string(),
            "json".to_string(),
        ],
    )
    .await
    .map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "failed to load pod", "errMessage": error })),
        )
            .into_response()
    })?;

    if !selector_matches_pod(&deployment, &pod) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "pod is not selected by the requested deployment" })),
        )
            .into_response());
    }

    let has_container = json_at(&pod, &["status", "containerStatuses"])
        .and_then(Value::as_array)
        .is_some_and(|containers| {
            containers.iter().any(|container| {
                json_at_string(container, &["name"]).as_deref() == Some(query.container.as_str())
            })
        });
    if !has_container {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "container was not found on the selected pod" })),
        )
            .into_response());
    }

    Ok(TerminalTarget {
        namespace: query.namespace,
        deployment: query.deployment,
        pod: query.pod,
        container: query.container,
    })
}

fn terminal_page_html(target: &TerminalTarget) -> String {
    let namespace = html_escape(&target.namespace);
    let deployment = html_escape(&target.deployment);
    let pod = html_escape(&target.pod);
    let container = html_escape(&target.container);
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Bastion terminal</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css">
  <script defer src="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js"></script>
  <style>
    :root {{ color-scheme: dark; --bg: #0d1117; --line: #263244; --text: #e5edf7; --muted: #9aa7b7; }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; min-height: 100dvh; background: var(--bg); color: var(--text); font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    main {{ min-height: 100dvh; display: grid; grid-template-rows: auto minmax(0, 1fr); }}
    header {{ display: flex; justify-content: space-between; gap: 12px; align-items: center; padding: 12px 14px; border-bottom: 1px solid var(--line); background: #0f172a; }}
    h1 {{ margin: 0; font-size: 16px; }}
    p {{ margin: 3px 0 0; color: var(--muted); font-size: 12px; }}
    #status {{ color: var(--muted); font-size: 13px; }}
    #terminal {{ min-height: 0; padding: 10px; background: #05080d; }}
    #terminal .xterm {{ height: 100%; }}
  </style>
</head>
<body>
  <main>
    <header>
      <div>
        <h1>{deployment}</h1>
        <p>{namespace}/{pod}/{container}</p>
      </div>
      <span id="status">connecting</span>
    </header>
    <div id="terminal" aria-label="Bastion container terminal"></div>
  </main>
  <script>
    const statusNode = document.getElementById("status");
    const terminalNode = document.getElementById("terminal");
    let term;
    function write(value) {{
      if (term) term.write(String(value || ""));
    }}
    function connect() {{
      if (!window.Terminal) {{
        statusNode.textContent = "terminal assets failed";
        terminalNode.textContent = "Terminal assets failed to load.";
        return;
      }}
      term = new Terminal({{
        cursorBlink: true,
        convertEol: true,
        fontSize: 13,
        rows: 32,
        scrollback: 5000,
        theme: {{
          background: "#05080d",
          foreground: "#d5f5e3",
          cursor: "#7dd3fc",
          selectionBackground: "#1f6feb66"
        }}
      }});
      term.open(terminalNode);
      term.focus();
      const url = new URL("terminal/ws", window.location.href);
      url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      for (const [key, value] of new URLSearchParams(window.location.search)) {{
        url.searchParams.set(key, value);
      }}
      const socket = new WebSocket(url);
      socket.addEventListener("open", () => {{
        statusNode.textContent = "connected";
        term.onData((data) => {{
          if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({{ type: "input", data }}));
        }});
      }});
      socket.addEventListener("message", (event) => {{
        let message;
        try {{ message = JSON.parse(event.data); }} catch {{ write(String(event.data)); return; }}
        if (message.type === "terminal-output") write(String(message.data || ""));
        if (message.type === "terminal-status") statusNode.textContent = String(message.status || "status");
        if (message.type === "terminal-error") {{
          statusNode.textContent = "error";
          write("\r\n" + String(message.message || "terminal error") + "\r\n");
        }}
        if (message.type === "terminal-exit") statusNode.textContent = "closed";
      }});
      socket.addEventListener("close", () => {{ statusNode.textContent = "closed"; }});
      socket.addEventListener("error", () => {{ statusNode.textContent = "connection error"; }});
    }}
    window.addEventListener("load", connect);
  </script>
</body>
</html>"##
    )
}

async fn terminal_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TerminalQuery>,
) -> Result<Html<String>, Response> {
    require_auth(&headers, &state)?;
    let target = validate_terminal_target(&state.config, query).await?;
    Ok(Html(terminal_page_html(&target)))
}

async fn terminal_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TerminalQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let target = match validate_terminal_target(&state.config, query).await {
        Ok(target) => target,
        Err(response) => return response,
    };
    let kubectl_bin = state.config.kubectl_bin.clone();
    let script_bin = state.config.script_bin.clone();
    ws.on_upgrade(move |socket| handle_terminal_socket(socket, kubectl_bin, script_bin, target))
}

async fn send_terminal_json(socket: &mut WebSocket, payload: Value) -> bool {
    socket
        .send(Message::Text(payload.to_string()))
        .await
        .is_ok()
}

fn terminal_input(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => value
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Err(_) => text.to_string(),
    }
}

async fn handle_terminal_socket(
    mut socket: WebSocket,
    kubectl_bin: String,
    script_bin: String,
    target: TerminalTarget,
) {
    let _ = send_terminal_json(
        &mut socket,
        json!({
            "type": "terminal-status",
            "source": "dd-bastion",
            "status": "starting-shell",
            "namespace": target.namespace,
            "deployment": target.deployment,
            "pod": target.pod,
            "container": target.container,
            "atMs": now_millis(),
        }),
    )
    .await;

    let use_pty = executable_available(&script_bin).await;
    let mut command = if use_pty {
        let kubectl_command = format!(
            "{} -n {} exec -it {} -c {} -- /bin/sh -lc {}",
            shell_quote(&kubectl_bin),
            shell_quote(&target.namespace),
            shell_quote(&target.pod),
            shell_quote(&target.container),
            shell_quote(TERMINAL_SHELL)
        );
        let mut command = Command::new(&script_bin);
        command.args(["-q", "-f", "-e", "-c", &kubectl_command, "/dev/null"]);
        command
    } else {
        let mut command = Command::new(&kubectl_bin);
        command.args([
            "-n",
            &target.namespace,
            "exec",
            "-i",
            &target.pod,
            "-c",
            &target.container,
            "--",
            "/bin/sh",
            "-lc",
            TERMINAL_SHELL,
        ]);
        command
    };
    let mut child = match command
        .env("TERM", "xterm-256color")
        .env("COLUMNS", "120")
        .env("LINES", "32")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = send_terminal_json(
                &mut socket,
                json!({
                    "type": "terminal-error",
                    "source": "dd-bastion",
                    "message": format!("failed to start kubectl exec: {error}"),
                    "atMs": now_millis(),
                }),
            )
            .await;
            return;
        }
    };

    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => return,
    };
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return,
    };
    let mut stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => return,
    };
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut stdout_buf = [0_u8; 4096];
    let mut stderr_buf = [0_u8; 4096];

    let _ = send_terminal_json(
        &mut socket,
        json!({
            "type": "terminal-status",
            "source": "dd-bastion",
            "status": "connected",
            "transport": if use_pty { "pty-script-kubectl" } else { "kubectl-pipe-fallback" },
            "atMs": now_millis(),
        }),
    )
    .await;

    loop {
        tokio::select! {
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let data = terminal_input(&text);
                        if !data.is_empty() && stdin.write_all(data.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if stdin.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(bytes))) => {
                        let _ = socket.send(Message::Pong(bytes)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            read = stdout.read(&mut stdout_buf), if stdout_open => {
                match read {
                    Ok(0) => stdout_open = false,
                    Ok(n) => {
                        if !send_terminal_json(&mut socket, json!({
                            "type": "terminal-output",
                            "source": "dd-bastion",
                            "data": String::from_utf8_lossy(&stdout_buf[..n]).to_string(),
                            "atMs": now_millis(),
                        })).await {
                            break;
                        }
                    }
                    Err(_) => stdout_open = false,
                }
            }
            read = stderr.read(&mut stderr_buf), if stderr_open => {
                match read {
                    Ok(0) => stderr_open = false,
                    Ok(n) => {
                        if !send_terminal_json(&mut socket, json!({
                            "type": "terminal-output",
                            "source": "dd-bastion",
                            "data": String::from_utf8_lossy(&stderr_buf[..n]).to_string(),
                            "atMs": now_millis(),
                        })).await {
                            break;
                        }
                    }
                    Err(_) => stderr_open = false,
                }
            }
            status = child.wait() => {
                let (code, signal) = match status {
                    Ok(status) => (status.code(), None::<String>),
                    Err(error) => (None, Some(error.to_string())),
                };
                let _ = send_terminal_json(&mut socket, json!({
                    "type": "terminal-exit",
                    "source": "dd-bastion",
                    "code": code,
                    "signal": signal,
                    "atMs": now_millis(),
                })).await;
                break;
            }
        }

        if !stdout_open && !stderr_open {
            break;
        }
    }

    let _ = child.kill().await;
}

#[tokio::main]
async fn main() {
    let host = env_value("HOST", "0.0.0.0");
    let port = first_env(&["PORT", "BASTION_PORT"])
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("HOST and PORT must form a valid socket address");
    let state = AppState {
        config: Arc::new(config_from_env()),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/profile", get(profile))
        .route("/config", get(profile))
        .route("/kubeconfig", get(kubeconfig))
        .route("/runtime/deployments", get(runtime_deployments))
        .route("/deployments", get(runtime_deployments))
        .route("/terminal", get(terminal_page))
        .route("/terminal/ws", get(terminal_ws))
        .with_state(state);

    println!("{SERVICE_NAME} listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind bastion listener");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = signal::ctrl_c().await;
        })
        .await
        .expect("bastion server failed");
}
