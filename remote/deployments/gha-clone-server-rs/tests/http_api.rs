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
    sync::Mutex,
    task::JoinHandle,
    time::{sleep, Duration},
};

const SERVER_BINARY: &str = env!("CARGO_BIN_EXE_gha-clone-server");
const AUTH_SECRET: &str = "test-server-auth";
const WEBHOOK_SECRET: &str = "test-webhook-secret";
const WEBHOOK_DELIVERY: &str = "4f5f1f6e-68a6-4d95-90b4-c0a892938f0f";
const BUILD_AUTH: &str = "test-build-auth";
const REPOSITORY: &str = "owner/repo";
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

const SERVER_ENV_VARS: &[&str] = &[
    "HOST",
    "PORT",
    "GHA_CLONE_AUTH_SECRET",
    "GHA_CLONE_GITHUB_WEBHOOK_SECRET",
    "GHA_CLONE_GITHUB_TOKEN",
    "GHA_CLONE_BUILD_SERVER_URL",
    "GHA_CLONE_BUILD_SERVER_AUTH",
    "GHA_CLONE_ALLOWED_REPOSITORIES",
    "GHA_CLONE_WORKFLOW_RULES_JSON",
    "GHA_CLONE_EXECUTION_ENABLED",
    "GHA_CLONE_WEBHOOK_EXECUTION_ENABLED",
    "GHA_CLONE_MAX_WORKFLOW_BYTES",
    "GHA_CLONE_MAX_JOBS",
    "GHA_CLONE_MAX_STEPS_PER_JOB",
    "GHA_CLONE_BUILD_POLL_SECONDS",
    "GHA_CLONE_BUILD_TIMEOUT_SECONDS",
    "GHA_CLONE_MAX_RUNS",
    "GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS",
    "GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS",
    "GHA_CLONE_WEBHOOK_DELIVERY_TTL_SECONDS",
    "GHA_CLONE_MAX_WEBHOOK_DELIVERIES",
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
    for _ in 0..200 {
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

fn dormant_env() -> BTreeMap<&'static str, String> {
    let mut env = BTreeMap::new();
    env.insert("GHA_CLONE_AUTH_SECRET", AUTH_SECRET.to_string());
    env.insert("GHA_CLONE_ALLOWED_REPOSITORIES", REPOSITORY.to_string());
    env.insert("GHA_CLONE_EXECUTION_ENABLED", "false".to_string());
    env.insert("GHA_CLONE_WEBHOOK_EXECUTION_ENABLED", "false".to_string());
    env
}

fn plan_request(repository: &str, workflow_yaml: &str) -> Value {
    json!({
        "repository": repository,
        "revision": REVISION,
        "workflowPath": ".github/workflows/ci.yml",
        "workflowYaml": workflow_yaml
    })
}

fn rust_workflow() -> &'static str {
    r#"
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#
}

fn rust_node_workflow() -> &'static str {
    r#"
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
  node:
    needs: rust
    runs-on: ubuntu-latest
    steps:
      - run: npm test
"#
}

async fn response_json(response: reqwest::Response) -> (StatusCode, Value) {
    let status = response.status();
    let body = response.text().await.expect("response body");
    let value = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("response was not JSON ({error}): {body}"));
    (status, value)
}

async fn get_json(client: &Client, server: &ServerProcess, path: &str) -> (StatusCode, Value) {
    response_json(
        client
            .get(format!("{}{path}", server.base_url))
            .send()
            .await
            .expect("GET request"),
    )
    .await
}

async fn post_plan(
    client: &Client,
    server: &ServerProcess,
    auth: Option<&str>,
    body: Value,
) -> reqwest::Response {
    let mut request = client
        .post(format!("{}/v1/plans", server.base_url))
        .json(&body);
    if let Some(auth) = auth {
        request = request.header("x-server-auth", auth);
    }
    request.send().await.expect("plan request")
}

async fn post_run(
    client: &Client,
    server: &ServerProcess,
    workflow_yaml: &str,
) -> reqwest::Response {
    client
        .post(format!("{}/v1/runs", server.base_url))
        .header("x-server-auth", AUTH_SECRET)
        .json(&plan_request(REPOSITORY, workflow_yaml))
        .send()
        .await
        .expect("run request")
}

async fn get_run(client: &Client, server: &ServerProcess, id: &str) -> reqwest::Response {
    client
        .get(format!("{}/v1/runs/{id}", server.base_url))
        .header("x-server-auth", AUTH_SECRET)
        .send()
        .await
        .expect("run status request")
}

async fn wait_for_terminal_run(client: &Client, server: &ServerProcess, id: &str) -> Value {
    for _ in 0..250 {
        let (status, run) = response_json(get_run(client, server, id).await).await;
        assert_eq!(status, StatusCode::OK);
        match run["status"].as_str() {
            Some("succeeded") | Some("failed") => return run,
            Some("queued") | Some("running") => sleep(Duration::from_millis(10)).await,
            other => panic!("unexpected run status {other:?}: {run}"),
        }
    }
    panic!("run {id} did not reach a terminal state");
}

fn webhook_signature(body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(WEBHOOK_SECRET.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

async fn post_webhook(
    client: &Client,
    server: &ServerProcess,
    event: &str,
    body: &[u8],
    signature: Option<&str>,
) -> reqwest::Response {
    let mut request = client
        .post(format!("{}/webhooks/github", server.base_url))
        .header("content-type", "application/json")
        .header("x-github-event", event)
        .header("x-github-delivery", WEBHOOK_DELIVERY)
        .body(body.to_vec());
    if let Some(signature) = signature {
        request = request.header("x-hub-signature-256", signature);
    }
    request.send().await.expect("webhook request")
}

#[tokio::test]
async fn public_endpoints_describe_the_dormant_fail_closed_server() {
    let server = spawn_server(dormant_env()).await;
    let client = Client::new();

    let (status, descriptor) = get_json(&client, &server, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(descriptor["service"], "gha-clone-server");
    assert_eq!(descriptor["endpoints"]["plan"], "POST /v1/plans");

    let (status, health) = get_json(&client, &server, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["ok"], true);
    assert_eq!(health["executionEnabled"], false);
    assert_eq!(health["allowedRepositories"], 1);
    assert_eq!(health["runsRetained"], 0);
    assert_eq!(health["webhookDeliveriesRetained"], 0);

    let (status, readiness) = get_json(&client, &server, "/readyz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(readiness["executionReady"], true);

    let (status, capabilities) = get_json(&client, &server, "/v1/capabilities").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(capabilities["service"], "gha-clone-server");
    assert_eq!(capabilities["planSchemaVersion"], "gha-clone-plan.v1");
}

#[tokio::test]
async fn readiness_fails_when_execution_is_enabled_without_build_prerequisites() {
    let mut env = dormant_env();
    env.insert("GHA_CLONE_EXECUTION_ENABLED", "true".to_string());
    let server = spawn_server(env).await;
    let client = Client::new();

    let (status, readiness) = get_json(&client, &server, "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(readiness["ok"], false);
    assert_eq!(readiness["executionReady"], false);
}

#[tokio::test]
async fn planning_enforces_auth_allowlists_and_structural_rejection() {
    let server = spawn_server(dormant_env()).await;
    let client = Client::new();

    let response = post_plan(
        &client,
        &server,
        None,
        plan_request(REPOSITORY, rust_workflow()),
    )
    .await;
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");

    let response = post_plan(
        &client,
        &server,
        Some("wrong-secret"),
        plan_request(REPOSITORY, rust_workflow()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = post_plan(
        &client,
        &server,
        Some(AUTH_SECRET),
        plan_request("other/repo", rust_workflow()),
    )
    .await;
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["repository"], "other/repo");

    let response = post_plan(
        &client,
        &server,
        Some(AUTH_SECRET),
        plan_request(REPOSITORY, "jobs: {}"),
    )
    .await;
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "workflow plan rejected");
    assert!(body["reasons"][0]
        .as_str()
        .unwrap()
        .contains("at least one job"));

    let response = post_plan(
        &client,
        &server,
        Some(AUTH_SECRET),
        plan_request(REPOSITORY, rust_workflow()),
    )
    .await;
    let (status, plan) = response_json(response).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plan["repository"], REPOSITORY);
    assert_eq!(plan["independentExecutable"], true);
    assert_eq!(plan["jobs"][0]["independentProfile"], "rust-verify");
}

#[tokio::test]
async fn run_creation_remains_disabled_before_planning_or_dispatch() {
    let server = spawn_server(dormant_env()).await;
    let client = Client::new();

    let response = post_run(&client, &server, "not valid: [").await;
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "independent execution is disabled");

    let unknown_id = "00000000-0000-4000-8000-000000000000";
    let response = get_run(&client, &server, unknown_id).await;
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "run not found");
}

#[tokio::test]
async fn webhook_guards_reject_bad_inputs_before_any_github_fetch() {
    let client = Client::new();
    let body = serde_json::to_vec(&json!({
        "repository": { "full_name": REPOSITORY }
    }))
    .unwrap();

    let no_secret_server = spawn_server(dormant_env()).await;
    let response = post_webhook(&client, &no_secret_server, "issues", &body, None).await;
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["error"], "GitHub webhook secret is not configured");
    drop(no_secret_server);

    let mut env = dormant_env();
    env.insert(
        "GHA_CLONE_GITHUB_WEBHOOK_SECRET",
        WEBHOOK_SECRET.to_string(),
    );
    let server = spawn_server(env).await;

    let response = post_webhook(&client, &server, "issues", &body, None).await;
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"], "missing X-Hub-Signature-256");

    let response = post_webhook(&client, &server, "issues", &body, Some("sha256=00")).await;
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"], "invalid GitHub webhook signature");

    let signature = webhook_signature(&body);
    let response = client
        .post(format!("{}/webhooks/github", server.base_url))
        .header("content-type", "application/json")
        .header("x-github-event", "issues")
        .header("x-hub-signature-256", &signature)
        .body(body.clone())
        .send()
        .await
        .expect("webhook without delivery");
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"], "missing or invalid X-GitHub-Delivery UUID");

    let response = client
        .post(format!("{}/webhooks/github", server.base_url))
        .header("content-type", "application/json")
        .header("x-github-event", "issues")
        .header("x-hub-signature-256", &signature)
        .header("x-github-delivery", "not-a-uuid")
        .body(body.clone())
        .send()
        .await
        .expect("webhook with invalid delivery");
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"], "missing or invalid X-GitHub-Delivery UUID");

    let invalid_json = b"{";
    let signature = webhook_signature(invalid_json);
    let response = post_webhook(&client, &server, "issues", invalid_json, Some(&signature)).await;
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("invalid webhook JSON"));

    let missing_repository = serde_json::to_vec(&json!({ "after": REVISION })).unwrap();
    let signature = webhook_signature(&missing_repository);
    let response = post_webhook(
        &client,
        &server,
        "push",
        &missing_repository,
        Some(&signature),
    )
    .await;
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("repository.full_name"));

    let forbidden = serde_json::to_vec(&json!({
        "repository": { "full_name": "other/repo" },
        "after": REVISION
    }))
    .unwrap();
    let signature = webhook_signature(&forbidden);
    let response = post_webhook(&client, &server, "push", &forbidden, Some(&signature)).await;
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["repository"], "other/repo");

    let unsupported = serde_json::to_vec(&json!({
        "repository": { "full_name": REPOSITORY }
    }))
    .unwrap();
    let signature = webhook_signature(&unsupported);
    let response = post_webhook(&client, &server, "issues", &unsupported, Some(&signature)).await;
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(value["accepted"], false);

    let short_revision = serde_json::to_vec(&json!({
        "repository": { "full_name": REPOSITORY },
        "after": "abc123"
    }))
    .unwrap();
    let signature = webhook_signature(&short_revision);
    let response = post_webhook(&client, &server, "push", &short_revision, Some(&signature)).await;
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(value["error"], "webhook revision is not a full commit SHA");

    let no_rules = serde_json::to_vec(&json!({
        "repository": { "full_name": REPOSITORY },
        "after": REVISION
    }))
    .unwrap();
    let signature = webhook_signature(&no_rules);
    let response = post_webhook(&client, &server, "push", &no_rules, Some(&signature)).await;
    let (status, value) = response_json(response).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(value["accepted"], false);
    assert!(value["reason"]
        .as_str()
        .unwrap()
        .contains("no workflow mirror rules"));
}

#[derive(Clone, Copy, Debug)]
enum MockMode {
    Succeed,
    RejectSubmission,
    InvalidSubmissionJson,
    FailBuild,
    UnknownStatus,
    InvalidStatusJson,
    KeepRunning,
}

#[derive(Clone)]
struct MockBuildState {
    mode: MockMode,
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
    async fn start(mode: MockMode) -> Self {
        let state = MockBuildState {
            mode,
            submissions: Arc::new(Mutex::new(Vec::new())),
            auth_headers: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/builds", post(mock_submit))
            .route("/builds/:id", get(mock_status))
            .with_state(state.clone());
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock build server");
        let address = listener.local_addr().expect("mock address");
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

async fn mock_submit(
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

    match state.mode {
        MockMode::RejectSubmission => {
            (StatusCode::INTERNAL_SERVER_ERROR, "simulated rejection").into_response()
        }
        MockMode::InvalidSubmissionJson => (StatusCode::ACCEPTED, "not-json").into_response(),
        _ => {
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
    }
}

async fn mock_status(State(state): State<MockBuildState>, Path(id): Path<String>) -> Response {
    match state.mode {
        MockMode::FailBuild => (
            StatusCode::OK,
            Json(json!({
                "id": id,
                "status": "failed",
                "error": "simulated failure"
            })),
        )
            .into_response(),
        MockMode::UnknownStatus => (
            StatusCode::OK,
            Json(json!({ "id": id, "status": "mystery", "error": null })),
        )
            .into_response(),
        MockMode::InvalidStatusJson => (StatusCode::OK, "not-json").into_response(),
        MockMode::KeepRunning => (
            StatusCode::OK,
            Json(json!({ "id": id, "status": "running", "error": null })),
        )
            .into_response(),
        _ => (
            StatusCode::OK,
            Json(json!({ "id": id, "status": "succeeded", "error": null })),
        )
            .into_response(),
    }
}

fn execution_env(mock: &MockBuildServer, timeout_seconds: u64) -> BTreeMap<&'static str, String> {
    let mut env = dormant_env();
    env.insert("GHA_CLONE_EXECUTION_ENABLED", "true".to_string());
    env.insert("GHA_CLONE_BUILD_SERVER_URL", mock.base_url.clone());
    env.insert("GHA_CLONE_BUILD_SERVER_AUTH", BUILD_AUTH.to_string());
    env.insert("GHA_CLONE_BUILD_POLL_SECONDS", "0".to_string());
    env.insert(
        "GHA_CLONE_BUILD_TIMEOUT_SECONDS",
        timeout_seconds.to_string(),
    );
    env
}

#[tokio::test]
async fn live_execution_dispatches_fixed_profiles_in_topological_order() {
    let mock = MockBuildServer::start(MockMode::Succeed).await;
    let server = spawn_server(execution_env(&mock, 5)).await;
    let client = Client::new();

    let response = post_run(&client, &server, rust_node_workflow()).await;
    let (status, accepted) = response_json(response).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(accepted["status"], "queued");
    let run_id = accepted["id"].as_str().expect("run id");

    let run = wait_for_terminal_run(&client, &server, run_id).await;
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["currentJob"], Value::Null);
    assert_eq!(run["submissions"].as_array().unwrap().len(), 2);
    assert_eq!(run["submissions"][0]["jobId"], "rust");
    assert_eq!(run["submissions"][0]["profile"], "rust-verify");
    assert_eq!(run["submissions"][0]["status"], "succeeded");
    assert_eq!(run["submissions"][1]["jobId"], "node");
    assert_eq!(run["submissions"][1]["profile"], "node-verify");

    let submissions = mock.state.submissions.lock().await.clone();
    assert_eq!(submissions.len(), 2);
    assert_eq!(submissions[0]["schemaVersion"], "build-server.v1");
    assert_eq!(submissions[0]["jobKind"], "run-profile");
    assert_eq!(
        submissions[0]["repoUrl"],
        "https://github.com/owner/repo.git"
    );
    assert_eq!(submissions[0]["gitRef"], REVISION);
    assert_eq!(submissions[0]["profile"], "rust-verify");
    assert!(submissions[0]["requestId"]
        .as_str()
        .unwrap()
        .ends_with(":rust"));
    assert_eq!(submissions[1]["profile"], "node-verify");
    assert!(submissions[1]["requestId"]
        .as_str()
        .unwrap()
        .ends_with(":node"));

    let auth_headers = mock.state.auth_headers.lock().await.clone();
    assert_eq!(
        auth_headers,
        vec![Some(BUILD_AUTH.into()), Some(BUILD_AUTH.into())]
    );
}

#[tokio::test]
async fn asynchronous_execution_failures_are_persisted_and_observable() {
    let cases = [
        (
            MockMode::RejectSubmission,
            2,
            "build server rejected rust with HTTP 500",
        ),
        (
            MockMode::InvalidSubmissionJson,
            2,
            "build server returned invalid job JSON",
        ),
        (MockMode::FailBuild, 2, "ended as failed: simulated failure"),
        (MockMode::UnknownStatus, 2, "unknown status"),
        (
            MockMode::InvalidStatusJson,
            2,
            "build status JSON is invalid",
        ),
        (MockMode::KeepRunning, 0, "exceeded 0 seconds"),
    ];

    for (mode, timeout, expected) in cases {
        let mock = MockBuildServer::start(mode).await;
        let server = spawn_server(execution_env(&mock, timeout)).await;
        let client = Client::new();

        let response = post_run(&client, &server, rust_workflow()).await;
        let (status, accepted) = response_json(response).await;
        assert_eq!(status, StatusCode::ACCEPTED, "mode {mode:?}");
        let run_id = accepted["id"].as_str().expect("run id");
        let run = wait_for_terminal_run(&client, &server, run_id).await;
        assert_eq!(run["status"], "failed", "mode {mode:?}: {run}");
        assert!(
            run["error"].as_str().unwrap().contains(expected),
            "mode {mode:?} expected {expected:?}: {run}"
        );
    }
}
