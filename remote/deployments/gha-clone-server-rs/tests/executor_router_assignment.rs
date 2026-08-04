use std::{
    fs,
    net::TcpListener as StdTcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::{
    net::TcpListener,
    task::JoinHandle,
    time::{sleep, Duration},
};
use uuid::Uuid;

const ROUTER_BINARY: &str = env!("CARGO_BIN_EXE_gha-executor-router");
const ROUTER_AUTH: &str = "router-auth-secret-with-at-least-32-bytes";
const AWS_AUTH: &str = "aws-build-auth-secret-with-at-least-32-bytes";
const HETZNER_AUTH: &str = "hetzner-build-auth-secret-at-least-32-bytes";
const REVISION_A: &str = "0123456789abcdef0123456789abcdef01234567";
const REVISION_B: &str = "1123456789abcdef0123456789abcdef01234567";

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
    "GHA_EXECUTOR_ROUTER_MAX_ASSIGNMENTS",
    "GHA_EXECUTOR_ROUTER_PROBE_TIMEOUT_MS",
    "GHA_EXECUTOR_ROUTER_UPSTREAM_TIMEOUT_SECONDS",
];

#[derive(Clone)]
struct MockState {
    ready_status: Arc<AtomicU16>,
    post_status: Arc<AtomicU16>,
    post_delay_ms: Arc<AtomicU64>,
    post_count: Arc<AtomicUsize>,
    accepted_id: String,
}

impl MockState {
    fn new(id: &str) -> Self {
        Self {
            ready_status: Arc::new(AtomicU16::new(StatusCode::OK.as_u16())),
            post_status: Arc::new(AtomicU16::new(StatusCode::ACCEPTED.as_u16())),
            post_delay_ms: Arc::new(AtomicU64::new(0)),
            post_count: Arc::new(AtomicUsize::new(0)),
            accepted_id: format!("{id}-job-550e8400-e29b-41d4-a716-446655440000"),
        }
    }

    fn set_ready(&self, status: StatusCode) {
        self.ready_status.store(status.as_u16(), Ordering::Relaxed);
    }

    fn set_post(&self, status: StatusCode) {
        self.post_status.store(status.as_u16(), Ordering::Relaxed);
    }

    fn set_post_delay(&self, delay_ms: u64) {
        self.post_delay_ms.store(delay_ms, Ordering::Relaxed);
    }
}

struct MockExecutor {
    state: MockState,
    base_url: String,
    task: JoinHandle<()>,
}

impl Drop for MockExecutor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_mock(id: &str) -> MockExecutor {
    let state = MockState::new(id);
    let app = Router::new()
        .route("/readyz", get(mock_ready))
        .route("/builds", post(mock_submit))
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

async fn mock_ready(State(state): State<MockState>) -> Response {
    let status = StatusCode::from_u16(state.ready_status.load(Ordering::Relaxed)).unwrap();
    (status, Json(json!({ "ok": status.is_success() }))).into_response()
}

async fn mock_submit(State(state): State<MockState>, Json(_request): Json<Value>) -> Response {
    state.post_count.fetch_add(1, Ordering::Relaxed);
    let delay_ms = state.post_delay_ms.load(Ordering::Relaxed);
    if delay_ms > 0 {
        sleep(Duration::from_millis(delay_ms)).await;
    }
    let status = StatusCode::from_u16(state.post_status.load(Ordering::Relaxed)).unwrap();
    if status == StatusCode::ACCEPTED {
        (
            status,
            Json(json!({ "id": state.accepted_id, "status": "queued" })),
        )
            .into_response()
    } else {
        (
            status,
            Json(json!({
                "error": "upstream-detail-must-not-be-forwarded",
                "token": "upstream-secret"
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
    std::env::temp_dir().join(format!("gha-executor-assignment-{}", Uuid::new_v4()))
}

async fn spawn_router(
    aws: &MockExecutor,
    hetzner: &MockExecutor,
    max_assignments: usize,
) -> RouterProcess {
    let port = unused_port();
    let root = secret_root();
    fs::create_dir_all(&root).unwrap();
    let router_auth = root.join("router-auth");
    let aws_auth = root.join("aws-auth");
    let hetzner_auth = root.join("hetzner-auth");
    fs::write(&router_auth, ROUTER_AUTH).unwrap();
    fs::write(&aws_auth, AWS_AUTH).unwrap();
    fs::write(&hetzner_auth, HETZNER_AUTH).unwrap();
    let specs = json!([
        {
            "id": "aws-primary",
            "provider": "aws",
            "enabled": true,
            "url": aws.base_url,
            "authPath": aws_auth
        },
        {
            "id": "hetzner-secondary",
            "provider": "hetzner",
            "enabled": true,
            "url": hetzner.base_url,
            "authPath": hetzner_auth
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
        .env("GHA_EXECUTOR_ROUTER_AUTH_PATH", &router_auth)
        .env("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON", specs.to_string())
        .env("GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED", "true")
        .env(
            "GHA_EXECUTOR_ROUTER_MAX_ASSIGNMENTS",
            max_assignments.to_string(),
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

fn build_request(request_id: &str, revision: &str) -> Value {
    json!({
        "schemaVersion": "build-server.v1",
        "jobKind": "run-profile",
        "repoUrl": "https://github.com/ORESoftware/k8s-cluster.git",
        "gitRef": revision,
        "profile": "rust-verify",
        "requestId": request_id
    })
}

async fn submit(base_url: &str, request: &Value) -> (StatusCode, Value, String) {
    let response = Client::new()
        .post(format!("{base_url}/builds"))
        .header("x-build-server-auth", ROUTER_AUTH)
        .json(request)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    let value = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, value, text)
}

#[tokio::test]
async fn sequential_and_concurrent_identical_requests_submit_once() {
    let aws = spawn_mock("aws").await;
    let hetzner = spawn_mock("hetzner").await;
    let router = spawn_router(&aws, &hetzner, 32).await;

    let request = build_request("assignment-sequential", REVISION_A);
    let first = submit(&router.base_url, &request).await;
    let second = submit(&router.base_url, &request).await;
    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(second.0, StatusCode::ACCEPTED);
    assert_eq!(first.1["id"], second.1["id"]);
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);

    aws.state.set_post_delay(150);
    let request = build_request("assignment-concurrent", REVISION_A);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let base_url = router.base_url.clone();
        let request = request.clone();
        handles.push(tokio::spawn(async move { submit(&base_url, &request).await }));
    }
    let mut route_id = None;
    for handle in handles {
        let result = handle.await.unwrap();
        assert_eq!(result.0, StatusCode::ACCEPTED);
        if let Some(expected) = route_id.as_ref() {
            assert_eq!(&result.1["id"], expected);
        } else {
            route_id = Some(result.1["id"].clone());
        }
    }
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 2);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn request_id_is_bound_to_immutable_request_content() {
    let aws = spawn_mock("aws").await;
    let hetzner = spawn_mock("hetzner").await;
    let router = spawn_router(&aws, &hetzner, 32).await;

    let first = build_request("assignment-conflict", REVISION_A);
    assert_eq!(submit(&router.base_url, &first).await.0, StatusCode::ACCEPTED);
    let changed = build_request("assignment-conflict", REVISION_B);
    let (status, body, text) = submit(&router.base_url, &changed).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"].as_str().unwrap().contains("different immutable"));
    assert!(!text.contains(REVISION_A));
    assert!(!text.contains(REVISION_B));
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn ambiguous_assignment_is_retained_and_retry_never_switches_provider() {
    let aws = spawn_mock("aws").await;
    let hetzner = spawn_mock("hetzner").await;
    aws.state.set_post(StatusCode::SERVICE_UNAVAILABLE);
    let router = spawn_router(&aws, &hetzner, 32).await;
    let request = build_request("assignment-ambiguous", REVISION_A);

    let first = submit(&router.base_url, &request).await;
    assert_eq!(first.0, StatusCode::BAD_GATEWAY);
    assert_eq!(first.1["executorId"], "aws-primary");
    aws.state.set_ready(StatusCode::SERVICE_UNAVAILABLE);
    let second = submit(&router.base_url, &request).await;
    assert_eq!(second.0, StatusCode::BAD_GATEWAY);
    assert_eq!(second.1["executorId"], "aws-primary");
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
    assert!(!first.2.contains("upstream-secret"));
    assert!(!second.2.contains("upstream-secret"));
}

#[tokio::test]
async fn fixed_rejection_is_retained_without_cross_provider_retry() {
    let aws = spawn_mock("aws").await;
    let hetzner = spawn_mock("hetzner").await;
    aws.state.set_post(StatusCode::BAD_REQUEST);
    let router = spawn_router(&aws, &hetzner, 32).await;
    let request = build_request("assignment-rejected", REVISION_A);

    let first = submit(&router.base_url, &request).await;
    assert_eq!(first.0, StatusCode::BAD_REQUEST);
    aws.state.set_ready(StatusCode::SERVICE_UNAVAILABLE);
    let second = submit(&router.base_url, &request).await;
    assert_eq!(second.0, StatusCode::BAD_REQUEST);
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn assignment_capacity_fails_closed_before_untracked_submission() {
    let aws = spawn_mock("aws").await;
    let hetzner = spawn_mock("hetzner").await;
    let router = spawn_router(&aws, &hetzner, 1).await;

    let first = build_request("assignment-capacity-1", REVISION_A);
    assert_eq!(submit(&router.base_url, &first).await.0, StatusCode::ACCEPTED);
    let second = build_request("assignment-capacity-2", REVISION_A);
    let (status, body, _) = submit(&router.base_url, &second).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["submissionAttempted"], false);
    assert_eq!(body["retryable"], false);
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn duplicate_auth_authorities_fail_closed() {
    let aws = spawn_mock("aws").await;
    let hetzner = spawn_mock("hetzner").await;
    let router = spawn_router(&aws, &hetzner, 32).await;
    let response = Client::new()
        .post(format!("{}/builds", router.base_url))
        .header("x-build-server-auth", ROUTER_AUTH)
        .header("x-server-auth", ROUTER_AUTH)
        .json(&build_request("assignment-auth", REVISION_A))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(aws.state.post_count.load(Ordering::Relaxed), 0);
    assert_eq!(hetzner.state.post_count.load(Ordering::Relaxed), 0);
}
