use std::{
    fs,
    net::TcpListener as StdTcpListener,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::Arc,
};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tokio::{
    net::TcpListener,
    sync::Mutex,
    time::{sleep, Duration, Instant},
};

const SERVER_AUTH: &str = "meta-server-auth";
const BUILD_AUTH: &str = "meta-build-auth";
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

#[derive(Clone, Default)]
struct MockBuildState {
    submissions: Arc<Mutex<Vec<Value>>>,
}

async fn submit_build(
    State(state): State<MockBuildState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    let authorized = headers
        .get("x-build-server-auth")
        .and_then(|value| value.to_str().ok())
        == Some(BUILD_AUTH);
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        );
    }

    state.submissions.lock().await.push(request);
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "id": "meta-build-1",
            "status": "queued",
            "error": null
        })),
    )
}

async fn get_build(Path(id): Path<String>) -> impl IntoResponse {
    if id != "meta-build-1" {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" })));
    }
    (
        StatusCode::OK,
        Json(json!({
            "id": id,
            "status": "succeeded",
            "error": null
        })),
    )
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reserve_port() -> u16 {
    let listener = StdTcpListener::bind(("127.0.0.1", 0)).expect("reserve server port");
    listener.local_addr().expect("local address").port()
}

async fn wait_until_ready(client: &reqwest::Client, base_url: &str, child: &mut ChildGuard) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().expect("inspect server process") {
            panic!("gha-clone-server exited before readiness with {status}");
        }
        if let Ok(response) = client.get(format!("{base_url}/readyz")).send().await {
            if response.status() == StatusCode::OK {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "gha-clone-server did not become ready"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_server_submits_its_own_workflow_to_the_fixed_build_profile() {
    let mock_state = MockBuildState::default();
    let mock_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock build server");
    let mock_address = mock_listener.local_addr().expect("mock address");
    let mock_app = Router::new()
        .route("/builds", post(submit_build))
        .route("/builds/:id", get(get_build))
        .with_state(mock_state.clone());
    let mock_task = tokio::spawn(async move {
        axum::serve(mock_listener, mock_app)
            .await
            .expect("mock build server");
    });

    let server_port = reserve_port();
    let server_url = format!("http://127.0.0.1:{server_port}");
    let binary = option_env!("CARGO_BIN_EXE_gha-clone-server")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_gha-clone-server").map(PathBuf::from))
        .expect("Cargo must expose the gha-clone-server binary to integration tests");
    let child = Command::new(binary)
        .env("HOST", "127.0.0.1")
        .env("PORT", server_port.to_string())
        .env("RUST_LOG", "error")
        .env("GHA_CLONE_AUTH_SECRET", SERVER_AUTH)
        .env("GHA_CLONE_EXECUTION_ENABLED", "true")
        .env("GHA_CLONE_WEBHOOK_EXECUTION_ENABLED", "false")
        .env("GHA_CLONE_ALLOWED_REPOSITORIES", "ORESoftware/k8s-cluster")
        .env("GHA_CLONE_WORKFLOW_RULES_JSON", "{}")
        .env(
            "GHA_CLONE_BUILD_SERVER_URL",
            format!("http://{mock_address}"),
        )
        .env("GHA_CLONE_BUILD_SERVER_AUTH", BUILD_AUTH)
        .env("GHA_CLONE_BUILD_POLL_SECONDS", "1")
        .env("GHA_CLONE_BUILD_TIMEOUT_SECONDS", "15")
        .env("GHA_CLONE_MAX_RUNS", "8")
        .env_remove("GHA_CLONE_GITHUB_TOKEN")
        .env_remove("GHA_CLONE_GITHUB_TOKEN_FILE")
        .env_remove("GHA_CLONE_GITHUB_WEBHOOK_SECRET")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start gha-clone-server");
    let mut child = ChildGuard { child };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP client");
    wait_until_ready(&client, &server_url, &mut child).await;

    let workflow_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.github/workflows/gha-clone-server-meta.yml");
    let workflow_yaml = fs::read_to_string(&workflow_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", workflow_path.display()));
    let response = client
        .post(format!("{server_url}/v1/runs"))
        .header("x-gha-clone-auth", SERVER_AUTH)
        .json(&json!({
            "repository": "ORESoftware/k8s-cluster",
            "revision": REVISION,
            "workflowPath": ".github/workflows/gha-clone-server-meta.yml",
            "workflowYaml": workflow_yaml
        }))
        .send()
        .await
        .expect("submit meta run");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let accepted: Value = response.json().await.expect("accepted run JSON");
    let run_id = accepted
        .get("id")
        .and_then(Value::as_str)
        .expect("accepted run id")
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(20);
    let final_run = loop {
        let response = client
            .get(format!("{server_url}/v1/runs/{run_id}"))
            .header("x-gha-clone-auth", SERVER_AUTH)
            .send()
            .await
            .expect("read meta run");
        assert_eq!(response.status(), StatusCode::OK);
        let run: Value = response.json().await.expect("run status JSON");
        match run.get("status").and_then(Value::as_str) {
            Some("succeeded") => break run,
            Some("failed") => panic!("meta self-test failed: {run}"),
            Some("queued" | "running") => {}
            other => panic!("unexpected meta self-test status: {other:?}"),
        }
        assert!(Instant::now() < deadline, "meta self-test timed out");
        sleep(Duration::from_millis(100)).await;
    };

    assert_eq!(final_run["repository"], "ORESoftware/k8s-cluster");
    assert_eq!(final_run["revision"], REVISION);
    assert_eq!(
        final_run["workflowPath"],
        ".github/workflows/gha-clone-server-meta.yml"
    );
    assert_eq!(final_run["submissions"][0]["profile"], "rust-verify");
    assert_eq!(final_run["submissions"][0]["status"], "succeeded");

    let submissions = mock_state.submissions.lock().await;
    assert_eq!(submissions.len(), 1);
    let submission = &submissions[0];
    assert_eq!(submission["schemaVersion"], "build-server.v1");
    assert_eq!(submission["jobKind"], "run-profile");
    assert_eq!(
        submission["repoUrl"],
        "https://github.com/ORESoftware/k8s-cluster.git"
    );
    assert_eq!(submission["gitRef"], REVISION);
    assert_eq!(submission["profile"], "rust-verify");
    assert!(submission["requestId"].as_str().is_some_and(
        |value| value.starts_with("gha-clone:") && value.ends_with(":gha-clone-self-test")
    ));

    mock_task.abort();
}
