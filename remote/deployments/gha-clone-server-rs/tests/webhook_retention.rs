use std::{
    net::TcpListener as StdTcpListener,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
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
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::{
    net::TcpListener,
    task::JoinHandle,
    time::{sleep, Duration},
};

const SERVER_BINARY: &str = env!("CARGO_BIN_EXE_gha-clone-server");
const AUTH_SECRET: &str = "retention-test-server-auth";
const WEBHOOK_SECRET: &str = "retention-test-webhook-secret";
const BUILD_AUTH: &str = "retention-test-build-auth";
const REPOSITORY: &str = "owner/repo";
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
const WORKFLOW_PATH: &str = ".github/workflows/ci.yml";

const SERVER_ENV_VARS: &[&str] = &[
    "HOST",
    "PORT",
    "GHA_CLONE_AUTH_SECRET",
    "GHA_CLONE_GITHUB_WEBHOOK_SECRET",
    "GHA_CLONE_GITHUB_TOKEN",
    "GHA_CLONE_GITHUB_API_BASE_URL",
    "GHA_CLONE_BUILD_SERVER_URL",
    "GHA_CLONE_BUILD_SERVER_AUTH",
    "GHA_CLONE_ALLOWED_REPOSITORIES",
    "GHA_CLONE_WORKFLOW_RULES_JSON",
    "GHA_CLONE_EXECUTION_ENABLED",
    "GHA_CLONE_WEBHOOK_EXECUTION_ENABLED",
    "GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS",
    "GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS",
    "GHA_CLONE_WEBHOOK_DELIVERY_TTL_SECONDS",
    "GHA_CLONE_MAX_WEBHOOK_DELIVERIES",
    "GHA_CLONE_BUILD_POLL_SECONDS",
    "GHA_CLONE_BUILD_TIMEOUT_SECONDS",
    "GHA_CLONE_MAX_RUNS",
];

struct ServerProcess {
    child: Child,
    base_url: String,
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn unused_port() -> u16 {
    StdTcpListener::bind(("127.0.0.1", 0))
        .expect("reserve local port")
        .local_addr()
        .expect("local address")
        .port()
}

async fn spawn_server(
    github_url: &str,
    build_url: &str,
    delivery_ttl_seconds: u64,
    max_deliveries: usize,
) -> ServerProcess {
    let port = unused_port();
    let mut command = Command::new(SERVER_BINARY);
    for &name in SERVER_ENV_VARS {
        command.env_remove(name);
    }
    command
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("RUST_LOG", "error")
        .env("GHA_CLONE_AUTH_SECRET", AUTH_SECRET)
        .env("GHA_CLONE_GITHUB_WEBHOOK_SECRET", WEBHOOK_SECRET)
        .env("GHA_CLONE_GITHUB_TOKEN", "retention-test-github-token")
        .env("GHA_CLONE_GITHUB_API_BASE_URL", github_url)
        .env("GHA_CLONE_BUILD_SERVER_URL", build_url)
        .env("GHA_CLONE_BUILD_SERVER_AUTH", BUILD_AUTH)
        .env("GHA_CLONE_ALLOWED_REPOSITORIES", REPOSITORY)
        .env(
            "GHA_CLONE_WORKFLOW_RULES_JSON",
            format!(r#"{{"{REPOSITORY}":["{WORKFLOW_PATH}"]}}"#),
        )
        .env("GHA_CLONE_EXECUTION_ENABLED", "true")
        .env("GHA_CLONE_WEBHOOK_EXECUTION_ENABLED", "true")
        .env("GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS", "failure")
        .env(
            "GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS",
            "GHA continuity server",
        )
        .env(
            "GHA_CLONE_WEBHOOK_DELIVERY_TTL_SECONDS",
            delivery_ttl_seconds.to_string(),
        )
        .env(
            "GHA_CLONE_MAX_WEBHOOK_DELIVERIES",
            max_deliveries.to_string(),
        )
        .env("GHA_CLONE_BUILD_POLL_SECONDS", "0")
        .env("GHA_CLONE_BUILD_TIMEOUT_SECONDS", "5")
        .env("GHA_CLONE_MAX_RUNS", "32")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().expect("start gha-clone-server binary");
    let mut server = ServerProcess {
        child,
        base_url: format!("http://127.0.0.1:{port}"),
    };
    wait_for_server(&mut server).await;
    server
}

async fn wait_for_server(server: &mut ServerProcess) {
    let client = Client::new();
    for _ in 0..250 {
        if let Some(status) = server.child.try_wait().expect("read child status") {
            panic!("gha-clone-server exited before readiness with {status}");
        }
        if let Ok(response) = client
            .get(format!("{}/healthz", server.base_url))
            .send()
            .await
        {
            if response.status() == StatusCode::OK {
                return;
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("gha-clone-server did not become ready");
}

async fn response_json(response: reqwest::Response) -> (StatusCode, Value) {
    let status = response.status();
    let body = response.text().await.expect("response body");
    let value = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("response was not JSON ({error}): {body}"));
    (status, value)
}

fn workflow_run_payload() -> Value {
    json!({
        "action": "completed",
        "repository": { "full_name": REPOSITORY },
        "workflow_run": {
            "name": "CI",
            "path": WORKFLOW_PATH,
            "head_sha": REVISION,
            "conclusion": "failure"
        }
    })
}

fn webhook_signature(body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(WEBHOOK_SECRET.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

async fn post_workflow_run(
    client: &Client,
    server: &ServerProcess,
    delivery: &str,
) -> (StatusCode, Value) {
    let body = serde_json::to_vec(&workflow_run_payload()).expect("serialize webhook payload");
    let signature = webhook_signature(&body);
    let response = client
        .post(format!("{}/webhooks/github", server.base_url))
        .header("content-type", "application/json")
        .header("x-github-event", "workflow_run")
        .header("x-github-delivery", delivery)
        .header("x-hub-signature-256", signature)
        .body(body)
        .send()
        .await
        .expect("workflow_run request");
    response_json(response).await
}

async fn retained_deliveries(client: &Client, server: &ServerProcess) -> usize {
    let response = client
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await
        .expect("health request");
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::OK);
    body["webhookDeliveriesRetained"]
        .as_u64()
        .expect("retained delivery count") as usize
}

#[derive(Clone)]
struct MockBuildState {
    submissions: Arc<AtomicUsize>,
    next_id: Arc<AtomicUsize>,
}

struct MockBuildServer {
    base_url: String,
    state: MockBuildState,
    task: JoinHandle<()>,
}

impl Drop for MockBuildServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockBuildServer {
    async fn start() -> Self {
        let state = MockBuildState {
            submissions: Arc::new(AtomicUsize::new(0)),
            next_id: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/builds", post(mock_build_submit))
            .route("/builds/:id", get(mock_build_status))
            .with_state(state.clone());
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock build server");
        let address = listener.local_addr().expect("build mock address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock build server");
        });
        Self {
            base_url: format!("http://{address}"),
            state,
            task,
        }
    }
}

async fn mock_build_submit(
    State(state): State<MockBuildState>,
    headers: HeaderMap,
    Json(_body): Json<Value>,
) -> Response {
    if headers
        .get("x-build-server-auth")
        .and_then(|value| value.to_str().ok())
        != Some(BUILD_AUTH)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.submissions.fetch_add(1, Ordering::SeqCst);
    let index = state.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "id": format!("build-{index}"),
            "status": "queued",
            "error": null
        })),
    )
        .into_response()
}

async fn mock_build_status(Path(id): Path<String>) -> Json<Value> {
    Json(json!({
        "id": id,
        "status": "succeeded",
        "error": null
    }))
}

struct MockGithub {
    base_url: String,
    task: JoinHandle<()>,
}

impl Drop for MockGithub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockGithub {
    async fn start() -> Self {
        let app = Router::new().route(
            "/repos/:owner/:repository/contents/*path",
            get(mock_github_contents),
        );
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock GitHub API");
        let address = listener.local_addr().expect("GitHub mock address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock GitHub API");
        });
        Self {
            base_url: format!("http://{address}"),
            task,
        }
    }
}

async fn mock_github_contents() -> &'static str {
    r#"
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#
}

async fn wait_for_submissions(build: &MockBuildServer, expected: usize) {
    for _ in 0..250 {
        if build.state.submissions.load(Ordering::SeqCst) >= expected {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "expected {expected} build submissions, observed {}",
        build.state.submissions.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn delivery_identity_can_be_dispatched_again_after_ttl_expiry() {
    let github = MockGithub::start().await;
    let build = MockBuildServer::start().await;
    let server = spawn_server(&github.base_url, &build.base_url, 1, 8).await;
    let client = Client::new();
    let delivery = "51111111-1111-4111-8111-111111111111";

    let (status, first) = post_workflow_run(&client, &server, delivery).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(first["accepted"], true);
    wait_for_submissions(&build, 1).await;
    assert_eq!(retained_deliveries(&client, &server).await, 1);

    sleep(Duration::from_millis(1_200)).await;

    let (status, replay) = post_workflow_run(&client, &server, delivery).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(replay["accepted"], true);
    assert_eq!(replay["delivery"], delivery);
    wait_for_submissions(&build, 2).await;
    assert_eq!(retained_deliveries(&client, &server).await, 1);
}

#[tokio::test]
async fn bounded_delivery_capacity_evicts_the_oldest_claim() {
    let github = MockGithub::start().await;
    let build = MockBuildServer::start().await;
    let server = spawn_server(&github.base_url, &build.base_url, 3_600, 1).await;
    let client = Client::new();
    let first_delivery = "61111111-1111-4111-8111-111111111111";
    let second_delivery = "62222222-2222-4222-8222-222222222222";

    let (status, first) = post_workflow_run(&client, &server, first_delivery).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(first["accepted"], true);
    wait_for_submissions(&build, 1).await;

    sleep(Duration::from_millis(20)).await;
    let (status, second) = post_workflow_run(&client, &server, second_delivery).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(second["accepted"], true);
    wait_for_submissions(&build, 2).await;
    assert_eq!(retained_deliveries(&client, &server).await, 1);

    let (status, replay) = post_workflow_run(&client, &server, first_delivery).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(replay["accepted"], true);
    wait_for_submissions(&build, 3).await;
    assert_eq!(retained_deliveries(&client, &server).await, 1);
}
