use std::{
    collections::BTreeMap,
    fs,
    net::TcpListener as StdTcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU16, AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::{
    net::TcpListener,
    sync::Mutex,
    task::JoinHandle,
    time::{sleep, Duration},
};
use uuid::Uuid;

const ROUTER_BINARY: &str = env!("CARGO_BIN_EXE_gha-executor-router");
const ROUTER_AUTH: &str = "router-auth-secret-with-at-least-32-bytes";
const AWS_AUTH: &str = "aws-build-auth-secret-with-at-least-32-bytes";
const HETZNER_AUTH: &str = "hetzner-build-auth-secret-at-least-32-bytes";
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

const ROUTER_ENV_VARS: &[&str] = &[
    "HOST",
    "PORT",
    "GHA_EXECUTOR_ROUTER_SECRET_ROOT",
    "GHA_EXECUTOR_ROUTER_AUTH_PATH",
    "GHA_EXECUTOR_ROUTER_EXECUTORS_JSON",
    "GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED",
    "GHA_EXECUTOR_ROUTER_MAX_EXECUTORS",
    "GHA_EXECUTOR_ROUTER_MAX_REQUEST_BYTES",
    "GHA_EXECUTOR_ROUTER_MAX_UPSTREAM_BODY_BYTES",
    "GHA_EXECUTOR_ROUTER_MAX_ERROR_CHARS",
    "GHA_EXECUTOR_ROUTER_PROBE_TIMEOUT_MS",
    "GHA_EXECUTOR_ROUTER_UPSTREAM_TIMEOUT_SECONDS",
];

#[derive(Clone)]
struct MockExecutorState {
    ready_status: Arc<AtomicU16>,
    post_status: Arc<AtomicU16>,
    get_status: Arc<AtomicU16>,
    post_count: Arc<AtomicUsize>,
    get_count: Arc<AtomicUsize>,
    received_auth: Arc<Mutex<Vec<String>>>,
    received_requests: Arc<Mutex<Vec<Value>>>,
    accepted_id: String,
    failure_marker: String,
}

impl MockExecutorState {
    fn new(accepted_id: &str, failure_marker: &str) -> Self {
        Self {
            ready_status: Arc::new(AtomicU16::new(StatusCode::OK.as_u16())),
            post_status: Arc::new(AtomicU16::new(StatusCode::ACCEPTED.as_u16())),
            get_status: Arc::new(AtomicU16::new(StatusCode::OK.as_u16())),
            post_count: Arc::new(AtomicUsize::new(0)),
            get_count: Arc::new(AtomicUsize::new(0)),
            received_auth: Arc::new(Mutex::new(Vec::new())),
            received_requests: Arc::new(Mutex::new(Vec::new())),
            accepted_id: accepted_id.to_string(),
            failure_marker: failure_marker.to_string(),
        }
    }

    fn set_ready(&self, status: StatusCode) {
        self.ready_status.store(status.as_u16(), Ordering::Relaxed);
    }

    fn set_post(&self, status: StatusCode) {
        self.post_status.store(status.as_u16(), Ordering::Relaxed);
    }

    fn set_get(&self, status: StatusCode) {
        self.get_status.store(status.as_u16(), Ordering::Relaxed);
    }
}

struct MockExecutor {
    state: MockExecutorState,
    base_url: String,
    task: JoinHandle<()>,
}

impl Drop for MockExecutor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_mock_executor(id: &str) -> MockExecutor {
    let state = MockExecutorState::new(
        &format!("{id}-job-550e8400-e29b-41d4-a716-446655440000"),
        &format!("{id}-upstream-secret-must-not-leak"),
    );
    let app = Router::new()
        .route("/readyz", get(mock_ready))
        .route("/builds", post(mock_submit))
        .route("/builds/:id", get(mock_status))
        .with_state(state.clone());
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    MockExecutor {
        state,
        base_url: format!("http://{address}"),
        task,
    }
}

async fn mock_ready(State(state): State<MockExecutorState>) -> Response {
    let status = StatusCode::from_u16(state.ready_status.load(Ordering::Relaxed)).unwrap();
    (
        status,
        Json(json!({
            "ok": status.is_success(),
            "service": "mock-dd-build-server"
        })),
    )
        .into_response()
}

async fn mock_submit(
    State(state): State<MockExecutorState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    state.post_count.fetch_add(1, Ordering::Relaxed);
    state.received_auth.lock().await.push(
        headers
            .get("x-build-server-auth")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string(),
    );
    state.received_requests.lock().await.push(request);
    let status = StatusCode::from_u16(state.post_status.load(Ordering::Relaxed)).unwrap();
    if status == StatusCode::ACCEPTED {
        (
            status,
            Json(json!({
                "id": state.accepted_id,
                "status": "queued"
            })),
        )
            .into_response()
    } else {
        (
            status,
            Json(json!({
                "error": state.failure_marker,
                "token": "upstream-body-secret"
            })),
        )
            .into_response()
    }
}

async fn mock_status(
    State(state): State<MockExecutorState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    state.get_count.fetch_add(1, Ordering::Relaxed);
    state.received_auth.lock().await.push(
        headers
            .get("x-build-server-auth")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string(),
    );
    let status = StatusCode::from_u16(state.get_status.load(Ordering::Relaxed)).unwrap();
    if status == StatusCode::OK {
        (
            status,
            Json(json!({
                "id": id,
                "status": "succeeded",
                "error": null
            })),
        )
            .into_response()
    } else {
        (
            status,
            Json(json!({
                "error": state.failure_marker,
                "token": "status-body-secret"
            })),
        )
            .into_response()
    }
}

struct RouterProcess {
    child: Child,
    base_url: String,
    secret_root: PathBuf,
}

impl Drop for RouterProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.secret_root);
    }
}

fn unused_port() -> u16 {
    StdTcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn secret_root() -> PathBuf {
    std::env::temp_dir().join(format!("gha-executor-router-test-{}", Uuid::new_v4()))
}

fn write_secrets(root: &PathBuf) -> BTreeMap<&'static str, PathBuf> {
    fs::create_dir_all(root).unwrap();
    let router = root.join("router-auth");
    let aws = root.join("aws-auth");
    let hetzner = root.join("hetzner-auth");
    fs::write(&router, ROUTER_AUTH).unwrap();
    fs::write(&aws, AWS_AUTH).unwrap();
    fs::write(&hetzner, HETZNER_AUTH).unwrap();
    BTreeMap::from([("router", router), ("aws", aws), ("hetzner", hetzner)])
}

async fn spawn_router(
    aws: &MockExecutor,
    hetzner: &MockExecutor,
    execution_enabled: bool,
) -> RouterProcess {
    let port = unused_port();
    let root = secret_root();
    let secrets = write_secrets(&root);
    let specs = json!([
        {
            "id": "aws-primary",
            "provider": "aws",
            "enabled": true,
            "url": aws.base_url,
            "authPath": secrets["aws"]
        },
        {
            "id": "hetzner-secondary",
            "provider": "hetzner",
            "enabled": true,
            "url": hetzner.base_url,
            "authPath": secrets["hetzner"]
        }
    ]);
    let mut command = Command::new(ROUTER_BINARY);
    for name in ROUTER_ENV_VARS {
        command.env_remove(name);
    }
    let child = command
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("RUST_LOG", "error")
        .env("GHA_EXECUTOR_ROUTER_SECRET_ROOT", &root)
        .env("GHA_EXECUTOR_ROUTER_AUTH_PATH", &secrets["router"])
        .env("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON", specs.to_string())
        .env(
            "GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED",
            execution_enabled.to_string(),
        )
        .env("GHA_EXECUTOR_ROUTER_PROBE_TIMEOUT_MS", "250")
        .env("GHA_EXECUTOR_ROUTER_UPSTREAM_TIMEOUT_SECONDS", "2")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut process = RouterProcess {
        child,
        base_url: format!("http://127.0.0.1:{port}"),
        secret_root: root,
    };
    wait_for_router(&mut process).await;
    process
}

async fn wait_for_router(process: &mut RouterProcess) {
    let client = Client::new();
    for _ in 0..200 {
        if let Some(status) = process.child.try_wait().unwrap() {
            panic!("router exited before readiness with {status}");
        }
        if client
            .get(format!("{}/healthz", process.base_url))
            .send()
            .await
            .is_ok_and(|response| response.status() == StatusCode::OK)
        {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("router did not become healthy");
}

fn build_request() -> Value {
    json!({
        "schemaVersion": "build-server.v1",
        "jobKind": "run-profile",
        "repoUrl": "https://github.com/ORESoftware/k8s-cluster.git",
        "gitRef": REVISION,
        "profile": "rust-verify",
        "requestId": "gha-clone:plan-1:rust"
    })
}

async fn submit(client: &Client, router: &RouterProcess, request: Value) -> reqwest::Response {
    client
        .post(format!("{}/builds", router.base_url))
        .header("x-build-server-auth", ROUTER_AUTH)
        .json(&request)
        .send()
        .await
        .unwrap()
}

async fn response_json(response: reqwest::Response) -> (StatusCode, Value, String) {
    let status = response.status();
    let text = response.text().await.unwrap();
    let value = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, value, text)
}

#[tokio::test]
async fn selects_first_ready_aws_executor_and_pins_status_to_it() {
    let aws = spawn_mock_executor("aws").await;
    let hetzner = spawn_mock_executor("hetzner").await;
    let router = spawn_router(&aws, &hetzner, true).await;
    let client = Client::new();

    let (status, accepted, _) =
        response_json(submit(&client, &router, build_request()).await).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(accepted["executorId"], "aws-primary");
    assert_eq!(accepted["provider"], "aws");
    let route_id = accepted["id"].as_str().unwrap();
    assert!(route_id.starts_with("aws-primary~aws-job-"));
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
    assert_eq!(
        aws.state.received_requests.lock().await[0]["requestId"],
        "gha-clone:plan-1:rust"
    );
    assert_eq!(aws.state.received_auth.lock().await[0], AWS_AUTH);

    let response = client
        .get(format!("{}/builds/{route_id}", router.base_url))
        .header("x-build-server-auth", ROUTER_AUTH)
        .send()
        .await
        .unwrap();
    let (status, build, _) = response_json(response).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(build["id"], route_id);
    assert_eq!(build["executorId"], "aws-primary");
    assert_eq!(build["provider"], "aws");
    assert_eq!(aws.state.get_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.get_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn readiness_failure_routes_to_hetzner_before_any_submission() {
    let aws = spawn_mock_executor("aws").await;
    let hetzner = spawn_mock_executor("hetzner").await;
    aws.state.set_ready(StatusCode::SERVICE_UNAVAILABLE);
    let router = spawn_router(&aws, &hetzner, true).await;
    let client = Client::new();

    let (status, accepted, _) =
        response_json(submit(&client, &router, build_request()).await).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(accepted["executorId"], "hetzner-secondary");
    assert_eq!(accepted["provider"], "hetzner");
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 0);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.received_auth.lock().await[0], HETZNER_AUTH);
}

#[tokio::test]
async fn explicit_rejection_does_not_submit_to_the_second_provider() {
    let aws = spawn_mock_executor("aws").await;
    let hetzner = spawn_mock_executor("hetzner").await;
    aws.state.set_post(StatusCode::BAD_REQUEST);
    let router = spawn_router(&aws, &hetzner, true).await;
    let client = Client::new();

    let (status, body, text) = response_json(submit(&client, &router, build_request()).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["automaticFailover"], false);
    assert_eq!(body["executorId"], "aws-primary");
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
    assert!(!text.contains(&aws.state.failure_marker));
    assert!(!text.contains("upstream-body-secret"));
}

#[tokio::test]
async fn ambiguous_submission_never_fails_over_or_leaks_upstream_body() {
    let aws = spawn_mock_executor("aws").await;
    let hetzner = spawn_mock_executor("hetzner").await;
    aws.state.set_post(StatusCode::SERVICE_UNAVAILABLE);
    let router = spawn_router(&aws, &hetzner, true).await;
    let client = Client::new();

    let (status, body, text) = response_json(submit(&client, &router, build_request()).await).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["automaticFailover"], false);
    assert_eq!(body["executorId"], "aws-primary");
    assert!(body["error"].as_str().unwrap().contains("ambiguous"));
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
    assert!(!text.contains(&aws.state.failure_marker));
    assert!(!text.contains("upstream-body-secret"));
}

#[tokio::test]
async fn accepted_build_status_failure_remains_pinned_without_resubmission() {
    let aws = spawn_mock_executor("aws").await;
    let hetzner = spawn_mock_executor("hetzner").await;
    let router = spawn_router(&aws, &hetzner, true).await;
    let client = Client::new();
    let (_, accepted, _) = response_json(submit(&client, &router, build_request()).await).await;
    let route_id = accepted["id"].as_str().unwrap();
    aws.state.set_get(StatusCode::SERVICE_UNAVAILABLE);

    let response = client
        .get(format!("{}/builds/{route_id}", router.base_url))
        .header("x-build-server-auth", ROUTER_AUTH)
        .send()
        .await
        .unwrap();
    let (status, body, text) = response_json(response).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["automaticFailover"], false);
    assert_eq!(body["executorId"], "aws-primary");
    assert_eq!(aws.state.get_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.get_count.load(Ordering::Relaxed), 0);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
    assert!(!text.contains(&aws.state.failure_marker));
    assert!(!text.contains("status-body-secret"));
}

#[tokio::test]
async fn auth_execution_and_fixed_profile_boundaries_fail_closed() {
    let aws = spawn_mock_executor("aws").await;
    let hetzner = spawn_mock_executor("hetzner").await;
    let router = spawn_router(&aws, &hetzner, true).await;
    let client = Client::new();

    let response = client
        .post(format!("{}/builds", router.base_url))
        .json(&build_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let mut arbitrary = build_request();
    arbitrary["image"] = json!("attacker/image:latest");
    let response = submit(&client, &router, arbitrary).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 0);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);

    let disabled = spawn_router(&aws, &hetzner, false).await;
    let response = submit(&client, &disabled, build_request()).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let ready = client
        .get(format!("{}/readyz", disabled.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
}

#[test]
fn malformed_startup_configuration_exits_before_binding() {
    let root = secret_root();
    let secrets = write_secrets(&root);
    let port = unused_port();
    let invalid = json!([
        {
            "id": "same",
            "provider": "aws",
            "enabled": true,
            "url": "http://127.0.0.1:1",
            "authPath": secrets["aws"]
        },
        {
            "id": "same",
            "provider": "hetzner",
            "enabled": true,
            "url": "http://127.0.0.1:2",
            "authPath": secrets["hetzner"]
        }
    ]);
    let output = Command::new(ROUTER_BINARY)
        .env_clear()
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("GHA_EXECUTOR_ROUTER_SECRET_ROOT", &root)
        .env("GHA_EXECUTOR_ROUTER_AUTH_PATH", &secrets["router"])
        .env("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON", invalid.to_string())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("duplicate executor id"));
    assert!(!stderr.contains(ROUTER_AUTH));
    assert!(!stderr.contains(AWS_AUTH));
    assert!(!stderr.contains(HETZNER_AUTH));
    fs::remove_dir_all(root).unwrap();
}
