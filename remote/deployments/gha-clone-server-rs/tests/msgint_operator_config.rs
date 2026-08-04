use std::{
    fs,
    net::TcpListener as StdTcpListener,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::Arc,
};

use axum::{
    extract::Path,
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

const SERVER_AUTH: &str = "msgint-server-auth";
const BUILD_AUTH: &str = "msgint-build-auth";
const REVISION: &str = "a9cc977d78347ec0efdbe8e6766967f80d425882";
const REPOSITORY: &str = "messaging-intel/msgint-connectors";
const WORKFLOW_PATH: &str = ".github/workflows/gha-clone-operator-config.yml";

#[derive(Clone, Default)]
struct MockBuildState {
    submissions: Arc<Mutex<Vec<Value>>>,
}

async fn submit_build(
    axum::extract::State(state): axum::extract::State<MockBuildState>,
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

    let mut submissions = state.submissions.lock().await;
    let build_id = format!("msgint-build-{}", submissions.len() + 1);
    submissions.push(request);
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "id": build_id,
            "status": "queued",
            "error": null
        })),
    )
}

async fn get_build(Path(id): Path<String>) -> impl IntoResponse {
    if !id.starts_with("msgint-build-") {
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
async fn running_server_dispatches_messaging_intel_operator_and_full_test_profiles() {
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
        .env("GHA_CLONE_ALLOWED_REPOSITORIES", REPOSITORY)
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

    let workflow_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/msgint-operator-config.yml");
    let workflow_yaml = fs::read_to_string(&workflow_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", workflow_path.display()));
    let response = client
        .post(format!("{server_url}/v1/runs"))
        .header("x-gha-clone-auth", SERVER_AUTH)
        .json(&json!({
            "repository": REPOSITORY,
            "revision": REVISION,
            "workflowPath": WORKFLOW_PATH,
            "workflowYaml": workflow_yaml
        }))
        .send()
        .await
        .expect("submit Messaging Intel run");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let accepted: Value = response.json().await.expect("accepted run JSON");
    let run_id = accepted
        .get("id")
        .and_then(Value::as_str)
        .expect("accepted run id")
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(25);
    let final_run = loop {
        let response = client
            .get(format!("{server_url}/v1/runs/{run_id}"))
            .header("x-gha-clone-auth", SERVER_AUTH)
            .send()
            .await
            .expect("read Messaging Intel run");
        assert_eq!(response.status(), StatusCode::OK);
        let run: Value = response.json().await.expect("run status JSON");
        match run.get("status").and_then(Value::as_str) {
            Some("succeeded") => break run,
            Some("failed") => panic!("Messaging Intel continuity run failed: {run}"),
            Some("queued" | "running") => {}
            other => panic!("unexpected Messaging Intel run status: {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "Messaging Intel continuity run timed out"
        );
        sleep(Duration::from_millis(100)).await;
    };

    assert_eq!(final_run["repository"], REPOSITORY);
    assert_eq!(final_run["revision"], REVISION);
    assert_eq!(final_run["workflowPath"], WORKFLOW_PATH);
    assert_eq!(final_run["submissions"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        final_run["submissions"][0]["profile"],
        "node-hardened-verify"
    );
    assert_eq!(final_run["submissions"][0]["status"], "succeeded");
    assert_eq!(final_run["submissions"][1]["profile"], "node-hardened-test");
    assert_eq!(final_run["submissions"][1]["status"], "succeeded");

    let submissions = mock_state.submissions.lock().await;
    assert_eq!(submissions.len(), 2);
    for submission in submissions.iter() {
        assert_eq!(submission["schemaVersion"], "build-server.v1");
        assert_eq!(submission["jobKind"], "run-profile");
        assert_eq!(
            submission["repoUrl"],
            "https://github.com/messaging-intel/msgint-connectors.git"
        );
        assert_eq!(submission["gitRef"], REVISION);
    }
    assert_eq!(submissions[0]["profile"], "node-hardened-verify");
    assert!(submissions[0]["requestId"]
        .as_str()
        .is_some_and(
            |value| value.starts_with("gha-clone:") && value.ends_with(":operator_config")
        ));
    assert_eq!(submissions[1]["profile"], "node-hardened-test");
    assert!(submissions[1]["requestId"]
        .as_str()
        .is_some_and(
            |value| value.starts_with("gha-clone:") && value.ends_with(":repository_tests")
        ));
    drop(submissions);

    let extra_command = workflow_yaml.replacen(
        "          npm audit --audit-level=high\n",
        "          npm audit --audit-level=high\n          npm publish\n",
        1,
    );
    let mutable_action = workflow_yaml.replacen(
        "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
        "actions/checkout@main",
        1,
    );
    let bracket_secret = workflow_yaml.replacen(
        "          persist-credentials: false\n",
        "          persist-credentials: false\n        env:\n          MSGINT_META_ACCESS_TOKEN: ${{ secrets['PROD_TOKEN'] }}\n",
        1,
    );

    for (label, rejected_yaml, expected_reason) in [
        (
            "extra hardened command",
            extra_command,
            "exact reviewed command sequence",
        ),
        (
            "mutable setup action",
            mutable_action,
            "exact 40-hex commit SHA",
        ),
        (
            "bracket secret expression",
            bracket_secret,
            "secret-bearing",
        ),
    ] {
        let response = client
            .post(format!("{server_url}/v1/runs"))
            .header("x-gha-clone-auth", SERVER_AUTH)
            .json(&json!({
                "repository": REPOSITORY,
                "revision": REVISION,
                "workflowPath": WORKFLOW_PATH,
                "workflowYaml": rejected_yaml
            }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("submit rejected {label} workflow: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label} unexpectedly reached execution"
        );
        let rejected: Value = response
            .json()
            .await
            .unwrap_or_else(|error| panic!("read rejected {label} response: {error}"));
        assert_eq!(
            rejected["error"],
            "workflow is not independently executable"
        );
        assert!(
            rejected.to_string().contains(expected_reason),
            "{label} response did not explain {expected_reason}: {rejected}"
        );
        assert_eq!(
            mock_state.submissions.lock().await.len(),
            2,
            "{label} dispatched a build despite rejection"
        );
    }

    mock_task.abort();
}
