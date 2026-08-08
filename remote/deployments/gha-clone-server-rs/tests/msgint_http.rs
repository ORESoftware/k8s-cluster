use std::{
    net::TcpListener as StdTcpListener,
    process::{Child, Command, Stdio},
    sync::Arc,
};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
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

const SERVER_BINARY: &str = env!("CARGO_BIN_EXE_gha-clone-server");
const AUTH_SECRET: &str = "msgint-test-server-auth";
const BUILD_AUTH: &str = "msgint-test-build-auth";
const REPOSITORY: &str = "messaging-intel/msgint-connectors";
const REVISION: &str = "a9cc977d78347ec0efdbe8e6766967f80d425882";
const WORKFLOW_PATH: &str = ".github/workflows/gha-clone-operator-config.yml";
const WORKFLOW: &str = include_str!("../fixtures/msgint-operator-config.yml");

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
    "GHA_CLONE_BUILD_POLL_SECONDS",
    "GHA_CLONE_BUILD_TIMEOUT_SECONDS",
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

#[derive(Clone)]
struct MockBuildState {
    submissions: Arc<Mutex<Vec<Value>>>,
    auth_headers: Arc<Mutex<Vec<Option<String>>>>,
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
        };
        let app = Router::new()
            .route("/builds", post(mock_submit))
            .route("/builds/:id", get(mock_status))
            .with_state(state.clone());
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind recording build server");
        let address = listener.local_addr().expect("recording server address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("recording build server");
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
) -> (StatusCode, Json<Value>) {
    let mut submissions = state.submissions.lock().await;
    submissions.push(body);
    let id = format!("msgint-build-{}", submissions.len());
    drop(submissions);
    state.auth_headers.lock().await.push(
        headers
            .get("x-build-server-auth")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    );
    (
        StatusCode::ACCEPTED,
        Json(json!({ "id": id, "status": "queued", "error": null })),
    )
}

async fn mock_status(Path(id): Path<String>) -> Json<Value> {
    Json(json!({ "id": id, "status": "succeeded", "error": null }))
}

async fn spawn_server(mock: &MockBuildServer) -> ServerProcess {
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
        .env("GHA_CLONE_ALLOWED_REPOSITORIES", REPOSITORY)
        .env("GHA_CLONE_EXECUTION_ENABLED", "true")
        .env("GHA_CLONE_WEBHOOK_EXECUTION_ENABLED", "false")
        .env("GHA_CLONE_BUILD_SERVER_URL", &mock.base_url)
        .env("GHA_CLONE_BUILD_SERVER_AUTH", BUILD_AUTH)
        .env("GHA_CLONE_BUILD_POLL_SECONDS", "1")
        .env("GHA_CLONE_BUILD_TIMEOUT_SECONDS", "5")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().expect("start real gha-clone-server binary");
    let mut server = ServerProcess {
        child,
        base_url: format!("http://127.0.0.1:{port}"),
    };
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
                return server;
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("gha-clone-server did not become ready");
}

fn run_request(repository: &str, revision: &str, workflow_path: &str, workflow: &str) -> Value {
    json!({
        "repository": repository,
        "revision": revision,
        "workflowPath": workflow_path,
        "workflowYaml": workflow,
    })
}

async fn post_run(
    client: &Client,
    server: &ServerProcess,
    repository: &str,
    revision: &str,
    workflow_path: &str,
    workflow: &str,
) -> reqwest::Response {
    client
        .post(format!("{}/v1/runs", server.base_url))
        .header("x-server-auth", AUTH_SECRET)
        .json(&run_request(repository, revision, workflow_path, workflow))
        .send()
        .await
        .expect("run request")
}

async fn response_json(response: reqwest::Response) -> (StatusCode, Value) {
    let status = response.status();
    let text = response.text().await.expect("response body");
    let value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("response was not JSON ({error}): {text}"));
    (status, value)
}

async fn wait_for_terminal_run(client: &Client, server: &ServerProcess, id: &str) -> Value {
    for _ in 0..250 {
        let response = client
            .get(format!("{}/v1/runs/{id}", server.base_url))
            .header("x-server-auth", AUTH_SECRET)
            .send()
            .await
            .expect("run status request");
        let (status, run) = response_json(response).await;
        assert_eq!(status, StatusCode::OK);
        match run["status"].as_str() {
            Some("succeeded") | Some("failed") => return run,
            Some("queued") | Some("running") => sleep(Duration::from_millis(10)).await,
            other => panic!("unexpected run status {other:?}: {run}"),
        }
    }
    panic!("run {id} did not reach a terminal state");
}

async fn run_to_success(client: &Client, server: &ServerProcess) -> Value {
    let response = post_run(
        client,
        server,
        REPOSITORY,
        REVISION,
        WORKFLOW_PATH,
        WORKFLOW,
    )
    .await;
    let (status, accepted) = response_json(response).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let id = accepted["id"].as_str().expect("run id");
    let run = wait_for_terminal_run(client, server, id).await;
    assert_eq!(run["status"], "succeeded", "{run}");
    run
}

async fn assert_rejected_without_dispatch(
    client: &Client,
    server: &ServerProcess,
    mock: &MockBuildServer,
    repository: &str,
    revision: &str,
    workflow_path: &str,
    workflow: &str,
    expected_status: StatusCode,
) {
    let response = post_run(
        client,
        server,
        repository,
        revision,
        workflow_path,
        workflow,
    )
    .await;
    let (status, _) = response_json(response).await;
    assert_eq!(status, expected_status);
    assert!(mock.state.submissions.lock().await.is_empty());
}

#[tokio::test]
async fn real_server_dispatches_exact_operator_then_repository_profiles() {
    let mock = MockBuildServer::start().await;
    let server = spawn_server(&mock).await;
    let client = Client::new();

    let run = run_to_success(&client, &server).await;
    let run_submissions = run["submissions"].as_array().expect("run submissions");
    assert_eq!(run_submissions.len(), 2);
    assert_eq!(run_submissions[0]["jobId"], "operator_config");
    assert_eq!(run_submissions[0]["profile"], "node-hardened-verify");
    assert_eq!(run_submissions[1]["jobId"], "repository_tests");
    assert_eq!(run_submissions[1]["profile"], "node-hardened-test");

    let submissions = mock.state.submissions.lock().await.clone();
    assert_eq!(submissions.len(), 2);
    for submission in &submissions {
        assert_eq!(submission["schemaVersion"], "build-server.v1");
        assert_eq!(submission["jobKind"], "run-profile");
        assert_eq!(
            submission["repoUrl"],
            format!("https://github.com/{REPOSITORY}.git")
        );
        assert_eq!(submission["gitRef"], REVISION);
        assert!(submission.get("command").is_none());
        assert!(submission.get("image").is_none());
    }

    let operator_id = submissions[0]["requestId"]
        .as_str()
        .expect("operator request id");
    let tests_id = submissions[1]["requestId"]
        .as_str()
        .expect("repository tests request id");
    assert!(operator_id.ends_with(":operator_config"));
    assert!(tests_id.ends_with(":repository_tests"));
    assert_eq!(
        operator_id.rsplit_once(':').expect("operator prefix").0,
        tests_id.rsplit_once(':').expect("tests prefix").0
    );
    assert_ne!(operator_id, tests_id);

    let auth_headers = mock.state.auth_headers.lock().await.clone();
    assert_eq!(auth_headers, vec![Some(BUILD_AUTH.to_string()); 2]);
}

#[tokio::test]
async fn exact_retry_reuses_each_deterministic_build_request_identity() {
    let mock = MockBuildServer::start().await;
    let server = spawn_server(&mock).await;
    let client = Client::new();

    run_to_success(&client, &server).await;
    run_to_success(&client, &server).await;

    let submissions = mock.state.submissions.lock().await.clone();
    assert_eq!(submissions.len(), 4);
    assert_eq!(submissions[0]["requestId"], submissions[2]["requestId"]);
    assert_eq!(submissions[1]["requestId"], submissions[3]["requestId"]);
    assert_ne!(submissions[0]["requestId"], submissions[1]["requestId"]);
}

#[tokio::test]
async fn reserved_identity_action_input_and_command_mutations_dispatch_nothing() {
    let mock = MockBuildServer::start().await;
    let server = spawn_server(&mock).await;
    let client = Client::new();

    assert_rejected_without_dispatch(
        &client,
        &server,
        &mock,
        REPOSITORY,
        "0000000000000000000000000000000000000000",
        WORKFLOW_PATH,
        WORKFLOW,
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
    assert_rejected_without_dispatch(
        &client,
        &server,
        &mock,
        REPOSITORY,
        REVISION,
        ".github/workflows/other.yml",
        WORKFLOW,
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
    assert_rejected_without_dispatch(
        &client,
        &server,
        &mock,
        "lookalike/msgint-connectors",
        REVISION,
        WORKFLOW_PATH,
        WORKFLOW,
        StatusCode::FORBIDDEN,
    )
    .await;

    for mutated in [
        WORKFLOW.replacen(
            "actions/setup-node@820762786026740c76f36085b0efc47a31fe5020",
            "actions/setup-node@main",
            1,
        ),
        WORKFLOW.replacen("persist-credentials: false", "persist-credentials: true", 1),
        WORKFLOW.replacen("node-version: \"22.23.1\"", "node-version: \"22\"", 1),
        WORKFLOW.replacen(
            "          npm audit --audit-level=high\n",
            "          npm audit --audit-level=high\n          npm publish\n",
            1,
        ),
        WORKFLOW.replacen(
            "          cache: npm\n",
            "          cache: npm\n          token: ${{ secrets.PROD_TOKEN }}\n",
            1,
        ),
    ] {
        assert_rejected_without_dispatch(
            &client,
            &server,
            &mock,
            REPOSITORY,
            REVISION,
            WORKFLOW_PATH,
            &mutated,
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
}
