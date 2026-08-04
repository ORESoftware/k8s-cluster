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
const AUTH_SECRET: &str = "streempilot-test-server-auth";
const BUILD_AUTH: &str = "streempilot-test-build-auth";
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
const WORKFLOW_PATH: &str = ".github/workflows/ci-mirror.yml";
const API_REPOSITORY: &str = "StreemPilot/streempilot-api-server.rs";
const WEB_REPOSITORY: &str = "StreemPilot/streempilot-web-server.rs";
const INTERFACES_REPOSITORY: &str = "StreemPilot/streempilot-interfaces";
const API_WORKFLOW: &str = include_str!("../fixtures/streempilot-api-ci-mirror.yml");
const WEB_WORKFLOW: &str = include_str!("../fixtures/streempilot-web-ci-mirror.yml");
const INTERFACES_WORKFLOW: &str = include_str!("../fixtures/streempilot-interfaces-ci-mirror.yml");

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
        .env(
            "GHA_CLONE_ALLOWED_REPOSITORIES",
            [API_REPOSITORY, WEB_REPOSITORY, INTERFACES_REPOSITORY].join(","),
        )
        .env("GHA_CLONE_EXECUTION_ENABLED", "true")
        .env("GHA_CLONE_WEBHOOK_EXECUTION_ENABLED", "false")
        .env("GHA_CLONE_BUILD_SERVER_URL", &mock.base_url)
        .env("GHA_CLONE_BUILD_SERVER_AUTH", BUILD_AUTH)
        .env("GHA_CLONE_BUILD_POLL_SECONDS", "0")
        .env("GHA_CLONE_BUILD_TIMEOUT_SECONDS", "5")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().expect("start real gha-clone-server binary");
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
    let id = format!("streempilot-build-{}", submissions.len());
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

fn run_request(repository: &str, revision: &str, workflow: &str) -> Value {
    json!({
        "repository": repository,
        "revision": revision,
        "workflowPath": WORKFLOW_PATH,
        "workflowYaml": workflow,
    })
}

async fn post_run(
    client: &Client,
    server: &ServerProcess,
    repository: &str,
    revision: &str,
    workflow: &str,
) -> reqwest::Response {
    client
        .post(format!("{}/v1/runs", server.base_url))
        .header("x-server-auth", AUTH_SECRET)
        .json(&run_request(repository, revision, workflow))
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

async fn run_to_success(
    client: &Client,
    server: &ServerProcess,
    repository: &str,
    workflow: &str,
) -> Value {
    let response = post_run(client, server, repository, REVISION, workflow).await;
    let (status, accepted) = response_json(response).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{repository}: {accepted}");
    let id = accepted["id"].as_str().expect("run id");
    let run = wait_for_terminal_run(client, server, id).await;
    assert_eq!(run["status"], "succeeded", "{repository}: {run}");
    run
}

#[tokio::test]
async fn real_server_dispatches_all_streempilot_mirrors_to_fixed_profiles() {
    let mock = MockBuildServer::start().await;
    let server = spawn_server(&mock).await;
    let client = Client::new();

    let api = run_to_success(&client, &server, API_REPOSITORY, API_WORKFLOW).await;
    let web = run_to_success(&client, &server, WEB_REPOSITORY, WEB_WORKFLOW).await;
    let interfaces =
        run_to_success(&client, &server, INTERFACES_REPOSITORY, INTERFACES_WORKFLOW).await;

    assert_eq!(api["submissions"].as_array().unwrap().len(), 1);
    assert_eq!(api["submissions"][0]["jobId"], "rust");
    assert_eq!(api["submissions"][0]["profile"], "rust-verify");
    assert_eq!(web["submissions"].as_array().unwrap().len(), 1);
    assert_eq!(web["submissions"][0]["profile"], "rust-verify");
    assert_eq!(interfaces["submissions"].as_array().unwrap().len(), 2);
    assert_eq!(interfaces["submissions"][0]["jobId"], "contracts");
    assert_eq!(interfaces["submissions"][0]["profile"], "node-verify");
    assert_eq!(interfaces["submissions"][1]["jobId"], "rust-bindings");
    assert_eq!(interfaces["submissions"][1]["profile"], "rust-verify");

    let submissions = mock.state.submissions.lock().await.clone();
    assert_eq!(submissions.len(), 4);
    assert_eq!(
        submissions[0]["repoUrl"],
        format!("https://github.com/{API_REPOSITORY}.git")
    );
    assert_eq!(
        submissions[1]["repoUrl"],
        format!("https://github.com/{WEB_REPOSITORY}.git")
    );
    assert_eq!(
        submissions[2]["repoUrl"],
        format!("https://github.com/{INTERFACES_REPOSITORY}.git")
    );
    assert_eq!(submissions[3]["repoUrl"], submissions[2]["repoUrl"]);
    for submission in &submissions {
        assert_eq!(submission["schemaVersion"], "build-server.v1");
        assert_eq!(submission["jobKind"], "run-profile");
        assert_eq!(submission["gitRef"], REVISION);
        assert!(submission.get("command").is_none());
        assert!(submission.get("image").is_none());
        assert!(submission["requestId"]
            .as_str()
            .is_some_and(|id| id.contains(':')));
    }

    let auth_headers = mock.state.auth_headers.lock().await.clone();
    assert_eq!(auth_headers, vec![Some(BUILD_AUTH.to_string()); 4]);
}

#[tokio::test]
async fn exact_retry_reuses_deterministic_build_request_identity() {
    let mock = MockBuildServer::start().await;
    let server = spawn_server(&mock).await;
    let client = Client::new();

    run_to_success(&client, &server, API_REPOSITORY, API_WORKFLOW).await;
    run_to_success(&client, &server, API_REPOSITORY, API_WORKFLOW).await;

    let submissions = mock.state.submissions.lock().await.clone();
    assert_eq!(submissions.len(), 2);
    assert_eq!(submissions[0]["requestId"], submissions[1]["requestId"]);
    assert_eq!(submissions[0]["gitRef"], submissions[1]["gitRef"]);
    assert_eq!(submissions[0]["profile"], submissions[1]["profile"]);
}

#[tokio::test]
async fn real_run_endpoint_rejects_mutable_revisions_and_unknown_repositories() {
    let mock = MockBuildServer::start().await;
    let server = spawn_server(&mock).await;
    let client = Client::new();

    let response = post_run(&client, &server, API_REPOSITORY, "main", API_WORKFLOW).await;
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "workflow is not independently executable");

    let response = post_run(
        &client,
        &server,
        "StreemPilot/unreviewed-repository",
        REVISION,
        API_WORKFLOW,
    )
    .await;
    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["repository"], "StreemPilot/unreviewed-repository");

    assert!(mock.state.submissions.lock().await.is_empty());
}
