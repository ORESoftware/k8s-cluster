use std::{
    collections::BTreeMap,
    net::TcpListener as StdTcpListener,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{
    extract::{Path, Query, State},
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
    sync::Mutex,
    task::JoinHandle,
    time::{sleep, Duration},
};

const SERVER_BINARY: &str = env!("CARGO_BIN_EXE_gha-clone-server");
const AUTH_SECRET: &str = "test-server-auth";
const WEBHOOK_SECRET: &str = "test-webhook-secret";
const GITHUB_TOKEN: &str = "test-github-token";
const BUILD_AUTH: &str = "test-build-auth";
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
    "GHA_CLONE_MAX_WORKFLOW_BYTES",
    "GHA_CLONE_MAX_JOBS",
    "GHA_CLONE_MAX_STEPS_PER_JOB",
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

async fn spawn_server(overrides: BTreeMap<&str, String>) -> ServerProcess {
    let port = unused_port();
    let mut command = Command::new(SERVER_BINARY);
    for &name in SERVER_ENV_VARS {
        command.env_remove(name);
    }
    command
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("RUST_LOG", "error")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, value) in overrides {
        command.env(name, value);
    }

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

fn rust_workflow() -> String {
    r#"
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#
    .to_string()
}

#[derive(Clone, Debug)]
struct GithubRequest {
    owner: String,
    repository: String,
    path: String,
    revision: Option<String>,
    accept: Option<String>,
    authorization: Option<String>,
}

#[derive(Clone)]
struct MockGithubState {
    workflow: Arc<String>,
    failures_remaining: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<GithubRequest>>>,
}

struct MockGithub {
    base_url: String,
    state: MockGithubState,
    task: JoinHandle<()>,
}

impl Drop for MockGithub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockGithub {
    async fn start(workflow: String, failures_before_success: usize) -> Self {
        let state = MockGithubState {
            workflow: Arc::new(workflow),
            failures_remaining: Arc::new(AtomicUsize::new(failures_before_success)),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route(
                "/repos/:owner/:repository/contents/*path",
                get(mock_github_contents),
            )
            .with_state(state.clone());
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock GitHub API");
        let address = listener.local_addr().expect("GitHub mock address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock GitHub API");
        });
        Self {
            base_url: format!("http://{address}"),
            state,
            task,
        }
    }
}

async fn mock_github_contents(
    State(state): State<MockGithubState>,
    Path((owner, repository, path)): Path<(String, String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    state.requests.lock().await.push(GithubRequest {
        owner,
        repository,
        path: path.trim_start_matches('/').to_string(),
        revision: query.get("ref").cloned(),
        accept: headers
            .get("accept")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        authorization: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    });

    if state.failures_remaining.load(Ordering::SeqCst) > 0 {
        state.failures_remaining.fetch_sub(1, Ordering::SeqCst);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "simulated transient GitHub failure",
        )
            .into_response();
    }

    (StatusCode::OK, state.workflow.as_str().to_string()).into_response()
}

#[derive(Clone)]
struct MockBuildState {
    submissions: Arc<Mutex<Vec<Value>>>,
    auth_headers: Arc<Mutex<Vec<Option<String>>>>,
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
            submissions: Arc::new(Mutex::new(Vec::new())),
            auth_headers: Arc::new(Mutex::new(Vec::new())),
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
    Json(body): Json<Value>,
) -> Response {
    state.submissions.lock().await.push(body);
    state.auth_headers.lock().await.push(
        headers
            .get("x-build-server-auth")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    );
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

fn execution_env(github: &MockGithub, build: &MockBuildServer) -> BTreeMap<&'static str, String> {
    let mut env = BTreeMap::new();
    env.insert("GHA_CLONE_AUTH_SECRET", AUTH_SECRET.to_string());
    env.insert(
        "GHA_CLONE_GITHUB_WEBHOOK_SECRET",
        WEBHOOK_SECRET.to_string(),
    );
    env.insert("GHA_CLONE_GITHUB_TOKEN", GITHUB_TOKEN.to_string());
    env.insert("GHA_CLONE_GITHUB_API_BASE_URL", github.base_url.clone());
    env.insert("GHA_CLONE_BUILD_SERVER_URL", build.base_url.clone());
    env.insert("GHA_CLONE_BUILD_SERVER_AUTH", BUILD_AUTH.to_string());
    env.insert("GHA_CLONE_ALLOWED_REPOSITORIES", REPOSITORY.to_string());
    env.insert(
        "GHA_CLONE_WORKFLOW_RULES_JSON",
        format!(r#"{{"{REPOSITORY}":["{WORKFLOW_PATH}"]}}"#),
    );
    env.insert("GHA_CLONE_EXECUTION_ENABLED", "true".to_string());
    env.insert("GHA_CLONE_WEBHOOK_EXECUTION_ENABLED", "true".to_string());
    env.insert(
        "GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS",
        "failure,timed_out".to_string(),
    );
    env.insert(
        "GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS",
        "GHA continuity server".to_string(),
    );
    env.insert("GHA_CLONE_WEBHOOK_DELIVERY_TTL_SECONDS", "3600".to_string());
    env.insert("GHA_CLONE_MAX_WEBHOOK_DELIVERIES", "32".to_string());
    env.insert("GHA_CLONE_BUILD_POLL_SECONDS", "0".to_string());
    env.insert("GHA_CLONE_BUILD_TIMEOUT_SECONDS", "5".to_string());
    env.insert("GHA_CLONE_MAX_RUNS", "32".to_string());
    env
}

fn workflow_run_payload(
    action: &str,
    conclusion: Option<&str>,
    workflow_name: &str,
    workflow_path: &str,
) -> Value {
    json!({
        "action": action,
        "repository": { "full_name": REPOSITORY },
        "workflow_run": {
            "name": workflow_name,
            "path": workflow_path,
            "head_sha": REVISION,
            "conclusion": conclusion
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
    payload: &Value,
) -> reqwest::Response {
    let body = serde_json::to_vec(payload).expect("serialize webhook payload");
    let signature = webhook_signature(&body);
    client
        .post(format!("{}/webhooks/github", server.base_url))
        .header("content-type", "application/json")
        .header("x-github-event", "workflow_run")
        .header("x-github-delivery", delivery)
        .header("x-hub-signature-256", signature)
        .body(body)
        .send()
        .await
        .expect("workflow_run request")
}

async fn get_run(client: &Client, server: &ServerProcess, id: &str) -> Value {
    let response = client
        .get(format!("{}/v1/runs/{id}", server.base_url))
        .header("x-server-auth", AUTH_SECRET)
        .send()
        .await
        .expect("run status request");
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::OK);
    body
}

async fn wait_for_terminal_run(client: &Client, server: &ServerProcess, id: &str) -> Value {
    for _ in 0..250 {
        let run = get_run(client, server, id).await;
        match run["status"].as_str() {
            Some("succeeded") | Some("failed") => return run,
            Some("queued") | Some("running") => sleep(Duration::from_millis(10)).await,
            other => panic!("unexpected run status {other:?}: {run}"),
        }
    }
    panic!("run {id} did not become terminal");
}

async fn health(client: &Client, server: &ServerProcess) -> Value {
    let response = client
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await
        .expect("health request");
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::OK);
    body
}

#[tokio::test]
async fn failed_workflow_fetches_exact_sha_dispatches_once_and_deduplicates() {
    let github = MockGithub::start(rust_workflow(), 0).await;
    let build = MockBuildServer::start().await;
    let server = spawn_server(execution_env(&github, &build)).await;
    let client = Client::new();

    let readiness = client
        .get(format!("{}/readyz", server.base_url))
        .send()
        .await
        .unwrap();
    let (status, readiness) = response_json(readiness).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(readiness["executionReady"], true);
    assert_eq!(readiness["webhookExecutionReady"], true);

    let payload = workflow_run_payload("completed", Some("failure"), "CI", WORKFLOW_PATH);
    let delivery = "11111111-1111-4111-8111-111111111111";
    let response = post_workflow_run(&client, &server, delivery, &payload).await;
    let (status, accepted) = response_json(response).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(accepted["accepted"], true);
    assert_eq!(accepted["delivery"], delivery);
    assert_eq!(accepted["runIds"].as_array().unwrap().len(), 1);

    let run_id = accepted["runIds"][0].as_str().expect("run id");
    let run = wait_for_terminal_run(&client, &server, run_id).await;
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["revision"], REVISION);
    assert_eq!(run["workflowPath"], WORKFLOW_PATH);

    let github_requests = github.state.requests.lock().await.clone();
    assert_eq!(github_requests.len(), 1);
    assert_eq!(github_requests[0].owner, "owner");
    assert_eq!(github_requests[0].repository, "repo");
    assert_eq!(github_requests[0].path, WORKFLOW_PATH);
    assert_eq!(github_requests[0].revision.as_deref(), Some(REVISION));
    assert_eq!(
        github_requests[0].accept.as_deref(),
        Some("application/vnd.github.raw+json")
    );
    assert_eq!(
        github_requests[0].authorization.as_deref(),
        Some("Bearer test-github-token")
    );

    let submissions = build.state.submissions.lock().await.clone();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0]["schemaVersion"], "build-server.v1");
    assert_eq!(submissions[0]["jobKind"], "run-profile");
    assert_eq!(submissions[0]["gitRef"], REVISION);
    assert_eq!(submissions[0]["profile"], "rust-verify");
    assert!(submissions[0]["requestId"]
        .as_str()
        .unwrap()
        .ends_with(":rust"));
    assert_eq!(
        build.state.auth_headers.lock().await.as_slice(),
        &[Some(BUILD_AUTH.to_string())]
    );

    let duplicate = post_workflow_run(&client, &server, delivery, &payload).await;
    let (status, duplicate) = response_json(duplicate).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(duplicate["accepted"], false);
    assert_eq!(duplicate["delivery"], delivery);
    assert!(duplicate["reason"]
        .as_str()
        .unwrap()
        .contains("duplicate GitHub delivery"));
    assert_eq!(github.state.requests.lock().await.len(), 2);
    assert_eq!(build.state.submissions.lock().await.len(), 1);

    let health = health(&client, &server).await;
    assert_eq!(health["webhookDeliveriesRetained"], 1);
    assert_eq!(health["webhookDeliveryTtlSeconds"], 3600);
    assert_eq!(health["maxWebhookDeliveries"], 32);
}

#[tokio::test]
async fn workflow_run_policy_rejections_never_fetch_or_dispatch() {
    let github = MockGithub::start(rust_workflow(), 0).await;
    let build = MockBuildServer::start().await;
    let server = spawn_server(execution_env(&github, &build)).await;
    let client = Client::new();

    let cases = [
        (
            "21111111-1111-4111-8111-111111111111",
            workflow_run_payload("in_progress", Some("failure"), "CI", WORKFLOW_PATH),
            "not the completed terminal phase",
        ),
        (
            "22222222-2222-4222-8222-222222222222",
            workflow_run_payload("completed", Some("success"), "CI", WORKFLOW_PATH),
            "not configured for failure fallback",
        ),
        (
            "23333333-3333-4333-8333-333333333333",
            workflow_run_payload(
                "completed",
                Some("failure"),
                "GHA continuity server",
                WORKFLOW_PATH,
            ),
            "excluded to prevent fallback recursion",
        ),
        (
            "24444444-4444-4444-8444-444444444444",
            workflow_run_payload(
                "completed",
                Some("failure"),
                "CI",
                ".github/workflows/release.yml",
            ),
            "is not configured for this repository",
        ),
        (
            "25555555-5555-4555-8555-555555555555",
            workflow_run_payload("completed", None, "CI", WORKFLOW_PATH),
            "missing a conclusion",
        ),
    ];

    for (delivery, payload, expected) in cases {
        let response = post_workflow_run(&client, &server, delivery, &payload).await;
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["accepted"], false);
        assert!(
            body["reason"].as_str().unwrap().contains(expected),
            "expected {expected:?}: {body}"
        );
    }

    assert!(github.state.requests.lock().await.is_empty());
    assert!(build.state.submissions.lock().await.is_empty());
    assert_eq!(
        health(&client, &server).await["webhookDeliveriesRetained"],
        0
    );
}

#[tokio::test]
async fn transient_github_failure_does_not_consume_delivery_identity() {
    let github = MockGithub::start(rust_workflow(), 1).await;
    let build = MockBuildServer::start().await;
    let server = spawn_server(execution_env(&github, &build)).await;
    let client = Client::new();

    let payload = workflow_run_payload("completed", Some("timed_out"), "CI", WORKFLOW_PATH);
    let delivery = "31111111-1111-4111-8111-111111111111";

    let first = post_workflow_run(&client, &server, delivery, &payload).await;
    let (status, first) = response_json(first).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(first["error"]
        .as_str()
        .unwrap()
        .contains("GitHub workflow fetch returned HTTP 500"));
    assert_eq!(
        health(&client, &server).await["webhookDeliveriesRetained"],
        0
    );
    assert!(build.state.submissions.lock().await.is_empty());

    let retry = post_workflow_run(&client, &server, delivery, &payload).await;
    let (status, retry) = response_json(retry).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(retry["accepted"], true);
    let run_id = retry["runIds"][0].as_str().unwrap();
    assert_eq!(
        wait_for_terminal_run(&client, &server, run_id).await["status"],
        "succeeded"
    );

    assert_eq!(github.state.requests.lock().await.len(), 2);
    assert_eq!(build.state.submissions.lock().await.len(), 1);
    assert_eq!(
        health(&client, &server).await["webhookDeliveriesRetained"],
        1
    );
}

#[tokio::test]
async fn concurrent_duplicate_deliveries_dispatch_exactly_once() {
    let github = MockGithub::start(rust_workflow(), 0).await;
    let build = MockBuildServer::start().await;
    let server = spawn_server(execution_env(&github, &build)).await;
    let client = Client::new();

    let payload = workflow_run_payload("completed", Some("failure"), "CI", WORKFLOW_PATH);
    let delivery = "41111111-1111-4111-8111-111111111111";
    let (left, right) = tokio::join!(
        post_workflow_run(&client, &server, delivery, &payload),
        post_workflow_run(&client, &server, delivery, &payload)
    );
    let (_, left) = response_json(left).await;
    let (_, right) = response_json(right).await;

    let accepted = [left, right]
        .into_iter()
        .filter(|body| body["accepted"] == true)
        .collect::<Vec<_>>();
    assert_eq!(accepted.len(), 1);
    let run_id = accepted[0]["runIds"][0].as_str().unwrap();
    assert_eq!(
        wait_for_terminal_run(&client, &server, run_id).await["status"],
        "succeeded"
    );

    assert_eq!(github.state.requests.lock().await.len(), 2);
    assert_eq!(build.state.submissions.lock().await.len(), 1);
    assert_eq!(
        health(&client, &server).await["webhookDeliveriesRetained"],
        1
    );
}

#[tokio::test]
async fn webhook_execution_readiness_requires_every_prerequisite() {
    let github = MockGithub::start(rust_workflow(), 0).await;
    let build = MockBuildServer::start().await;
    let client = Client::new();

    let mut execution_disabled = execution_env(&github, &build);
    execution_disabled.insert("GHA_CLONE_EXECUTION_ENABLED", "false".to_string());
    let server = spawn_server(execution_disabled).await;
    let response = client
        .get(format!("{}/readyz", server.base_url))
        .send()
        .await
        .unwrap();
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["executionReady"], true);
    assert_eq!(body["webhookExecutionReady"], false);
    drop(server);

    let mut missing_secret = execution_env(&github, &build);
    missing_secret.remove("GHA_CLONE_GITHUB_WEBHOOK_SECRET");
    let server = spawn_server(missing_secret).await;
    let response = client
        .get(format!("{}/readyz", server.base_url))
        .send()
        .await
        .unwrap();
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["executionReady"], true);
    assert_eq!(body["webhookExecutionReady"], false);
    drop(server);

    let mut missing_rules = execution_env(&github, &build);
    missing_rules.insert("GHA_CLONE_WORKFLOW_RULES_JSON", "{}".to_string());
    let server = spawn_server(missing_rules).await;
    let response = client
        .get(format!("{}/readyz", server.base_url))
        .send()
        .await
        .unwrap();
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["executionReady"], true);
    assert_eq!(body["webhookExecutionReady"], false);
}
