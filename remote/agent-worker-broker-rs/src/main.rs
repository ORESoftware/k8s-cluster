use std::{
    env, fs,
    net::SocketAddr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone)]
struct AppState {
    config: Config,
    http: reqwest::Client,
}

#[derive(Clone)]
struct Config {
    namespace: String,
    nats_url: String,
    nats_task_stream: String,
    nats_task_subject: String,
    nats_wakeup_subject: String,
    direct_dispatch_enabled: bool,
    wake_on_dispatch: bool,
    worker_health_timeout: Duration,
    worker_task_timeout: Duration,
    server_auth_secret: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchTaskRequest {
    task_id: String,
    thread_id: Option<String>,
    prompt: String,
    provider: Option<String>,
    thread_title: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrokerHealth {
    ok: bool,
    service: &'static str,
    direct_dispatch_enabled: bool,
    wake_on_dispatch: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NatsPublishResult {
    published: bool,
    subject: String,
    wakeup_subject: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerDispatchResult {
    attempted: bool,
    worker_awake: bool,
    sent: bool,
    status: Option<u16>,
    body: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct K8sWakeResult {
    attempted: bool,
    ok: bool,
    resource: String,
    status: Option<u16>,
    body: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrokerResponse {
    ok: bool,
    mode: &'static str,
    thread_id: String,
    task_id: String,
    worker_name: String,
    nats: NatsPublishResult,
    direct_dispatch: WorkerDispatchResult,
    wake: K8sWakeResult,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NatsTaskMessage {
    version: u8,
    message_kind: &'static str,
    task_kind: &'static str,
    shadow: bool,
    direct_dispatch: bool,
    thread_id: String,
    task_id: String,
    provider: Option<String>,
    repo: &'static str,
    base_branch: &'static str,
    feature_branch: Option<String>,
    prompt: String,
    created_at_ms: u128,
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
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"))
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    first_env(&[key])
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn thread_resource_name(thread_id: &str) -> String {
    let short = thread_id
        .chars()
        .filter(|value| value.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>()
        .to_lowercase();
    format!("dd-thread-{short}")
}

fn thread_worker_url(namespace: &str, name: &str, path: &str) -> String {
    format!("http://{name}.{namespace}.svc.cluster.local:8080{path}")
}

fn config_from_env() -> Config {
    Config {
        namespace: env_value("THREAD_RUNTIME_NAMESPACE", "default"),
        nats_url: env_value("NATS_URL", "nats://dd-nats.messaging.svc.cluster.local:4222"),
        nats_task_stream: env_value("NATS_TASK_STREAM", "DD_REMOTE_TASKS"),
        nats_task_subject: env_value("NATS_TASK_SUBJECT", "dd.remote.thread.*.tasks"),
        nats_wakeup_subject: env_value("NATS_WAKEUP_SUBJECT", "dd.remote.orchestrator.wakeup"),
        direct_dispatch_enabled: env_bool("DIRECT_DISPATCH_ENABLED", true),
        wake_on_dispatch: env_bool("WAKE_ON_DISPATCH", true),
        worker_health_timeout: Duration::from_millis(env_u64("WORKER_HEALTH_TIMEOUT_MS", 800)),
        worker_task_timeout: Duration::from_millis(env_u64("WORKER_TASK_TIMEOUT_MS", 30_000)),
        server_auth_secret: first_env(&["REMOTE_DEV_SERVER_SECRET", "SERVER_AUTH_SECRET"]),
    }
}

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == secret)
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    Json(BrokerHealth {
        ok: true,
        service: "dd-agent-worker-broker",
        direct_dispatch_enabled: state.config.direct_dispatch_enabled,
        wake_on_dispatch: state.config.wake_on_dispatch,
    })
}

async fn ensure_task_stream(config: &Config, client: async_nats::Client) -> Result<(), String> {
    let jetstream = async_nats::jetstream::new(client);
    jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: config.nats_task_stream.clone(),
            subjects: vec![config.nats_task_subject.clone()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            max_message_size: 8 * 1024 * 1024,
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn publish_task_to_nats(
    config: &Config,
    thread_id: &str,
    request: &DispatchTaskRequest,
) -> Result<NatsPublishResult, String> {
    let client = async_nats::connect(config.nats_url.clone())
        .await
        .map_err(|error| error.to_string())?;
    ensure_task_stream(config, client.clone()).await?;

    let subject = format!("dd.remote.thread.{thread_id}.tasks");
    let payload = serde_json::to_vec(&NatsTaskMessage {
        version: 1,
        message_kind: "task.dispatch",
        task_kind: "agent.prompt",
        shadow: false,
        direct_dispatch: false,
        thread_id: thread_id.to_string(),
        task_id: request.task_id.clone(),
        provider: request.provider.clone(),
        repo: "git@github.com:dancing-dragons/dd-next-1.git",
        base_branch: "dev",
        feature_branch: None,
        prompt: request.prompt.clone(),
        created_at_ms: now_ms(),
    })
    .map_err(|error| error.to_string())?;

    let jetstream = async_nats::jetstream::new(client.clone());
    jetstream
        .publish(subject.clone(), payload.clone().into())
        .await
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| error.to_string())?;
    client
        .publish(config.nats_wakeup_subject.clone(), payload.into())
        .await
        .map_err(|error| error.to_string())?;
    client.flush().await.map_err(|error| error.to_string())?;

    Ok(NatsPublishResult {
        published: true,
        subject,
        wakeup_subject: config.nats_wakeup_subject.clone(),
    })
}

async fn worker_is_awake(state: &AppState, worker_name: &str) -> bool {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        return false;
    };
    let url = thread_worker_url(&state.config.namespace, worker_name, "/healthz");
    state
        .http
        .get(url)
        .timeout(state.config.worker_health_timeout)
        .header("X-Server-Auth", secret)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

async fn direct_dispatch(
    state: &AppState,
    thread_id: &str,
    worker_name: &str,
    request: &DispatchTaskRequest,
) -> WorkerDispatchResult {
    if !state.config.direct_dispatch_enabled {
        return WorkerDispatchResult {
            attempted: false,
            worker_awake: false,
            sent: false,
            status: None,
            body: None,
            error: None,
        };
    }

    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        return WorkerDispatchResult {
            attempted: true,
            worker_awake: false,
            sent: false,
            status: None,
            body: None,
            error: Some("REMOTE_DEV_SERVER_SECRET or SERVER_AUTH_SECRET is not set".to_string()),
        };
    };

    if !worker_is_awake(state, worker_name).await {
        return WorkerDispatchResult {
            attempted: true,
            worker_awake: false,
            sent: false,
            status: None,
            body: None,
            error: None,
        };
    }

    let worker_body = json!({
        "taskId": &request.task_id,
        "threadId": thread_id,
        "prompt": &request.prompt,
        "provider": &request.provider,
        "threadTitle": &request.thread_title,
    });
    let url = thread_worker_url(&state.config.namespace, worker_name, "/tasks");
    match state
        .http
        .post(url)
        .timeout(state.config.worker_task_timeout)
        .header("X-Server-Auth", secret)
        .json(&worker_body)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            WorkerDispatchResult {
                attempted: true,
                worker_awake: true,
                sent: status.is_success(),
                status: Some(status.as_u16()),
                body: Some(body.chars().take(2_000).collect()),
                error: None,
            }
        }
        Err(error) => WorkerDispatchResult {
            attempted: true,
            worker_awake: true,
            sent: false,
            status: None,
            body: None,
            error: Some(error.to_string()),
        },
    }
}

async fn k8s_http_client() -> Result<(reqwest::Client, String, String), String> {
    let base_url = if let Some(value) = first_env(&["K8S_API_SERVER"]) {
        value
    } else {
        let host = env::var("KUBERNETES_SERVICE_HOST")
            .map_err(|_| "KUBERNETES_SERVICE_HOST is not set".to_string())?;
        let port = env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_string());
        format!("https://{host}:{port}")
    };
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
    Ok((client, base_url, token))
}

async fn wake_worker(config: &Config, worker_name: &str) -> K8sWakeResult {
    if !config.wake_on_dispatch {
        return K8sWakeResult {
            attempted: false,
            ok: false,
            resource: String::new(),
            status: None,
            body: None,
            error: None,
        };
    }

    let resource = format!(
        "/apis/apps/v1/namespaces/{}/deployments/{}/scale",
        config.namespace, worker_name
    );
    let (client, base_url, token) = match k8s_http_client().await {
        Ok(value) => value,
        Err(error) => {
            return K8sWakeResult {
                attempted: true,
                ok: false,
                resource,
                status: None,
                body: None,
                error: Some(error),
            }
        }
    };

    match client
        .patch(format!("{base_url}{resource}"))
        .bearer_auth(token.trim())
        .header("Accept", "application/json")
        .header("Content-Type", "application/merge-patch+json")
        .json(&json!({ "spec": { "replicas": 1 } }))
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            K8sWakeResult {
                attempted: true,
                ok: status.is_success(),
                resource,
                status: Some(status.as_u16()),
                body: Some(body.chars().take(1_000).collect()),
                error: None,
            }
        }
        Err(error) => K8sWakeResult {
            attempted: true,
            ok: false,
            resource,
            status: None,
            body: None,
            error: Some(error.to_string()),
        },
    }
}

async fn dispatch_task(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DispatchTaskRequest>,
) -> Response {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "SERVER_AUTH_SECRET is not configured" })),
        )
            .into_response();
    };
    if !request_is_authorized(&headers, secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "errMessage": "missing required worker broker auth header",
            })),
        )
            .into_response();
    }
    if let Some(body_thread_id) = request.thread_id.as_deref() {
        if body_thread_id != thread_id {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "threadId path/body mismatch" })),
            )
                .into_response();
        }
    }

    let worker_name = thread_resource_name(&thread_id);
    let nats = match publish_task_to_nats(&state.config, &thread_id, &request).await {
        Ok(value) => value,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "failed to publish agent task", "detail": error })),
            )
                .into_response();
        }
    };
    let direct_dispatch = direct_dispatch(&state, &thread_id, &worker_name, &request).await;
    let wake = if direct_dispatch.sent {
        K8sWakeResult {
            attempted: false,
            ok: false,
            resource: String::new(),
            status: None,
            body: None,
            error: None,
        }
    } else {
        wake_worker(&state.config, &worker_name).await
    };
    let mode = if direct_dispatch.sent {
        "direct-and-queued"
    } else {
        "queued"
    };
    let status = if direct_dispatch.sent {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };

    (
        status,
        Json(BrokerResponse {
            ok: true,
            mode,
            thread_id,
            task_id: request.task_id,
            worker_name,
            nats,
            direct_dispatch,
            wake,
        }),
    )
        .into_response()
}

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let config = config_from_env();
    let host = env_value("HOST", "0.0.0.0");
    let port = env_u64("PORT", 8098) as u16;
    let state = AppState {
        config,
        http: reqwest::Client::new(),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route(
            "/api/agent-worker/threads/:thread_id/tasks",
            post(dispatch_task),
        )
        .with_state(state);

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    println!("dd-agent-worker-broker listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let _ = sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
