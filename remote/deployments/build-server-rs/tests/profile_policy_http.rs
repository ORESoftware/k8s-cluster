use std::{
    fs,
    net::TcpListener as StdTcpListener,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration, Instant};

const AUTH_SECRET: &str = "profile-policy-http-test-secret";
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

struct ChildGuard {
    child: Child,
    work_root: PathBuf,
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
        let _ = fs::remove_dir_all(&self.work_root);
    }
}

fn reserve_port() -> u16 {
    let listener = StdTcpListener::bind(("127.0.0.1", 0)).expect("reserve build-server port");
    listener.local_addr().expect("local address").port()
}

fn unique_work_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "dd-build-server-profile-policy-http-{}-{nonce}",
        std::process::id()
    ))
}

async fn wait_until_healthy(
    client: &reqwest::Client,
    base_url: &str,
    child: &mut ChildGuard,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().expect("inspect build-server process") {
            panic!("dd-build-server exited before health check with {status}");
        }
        if let Ok(response) = client.get(format!("{base_url}/healthz")).send().await {
            if response.status() == StatusCode::OK {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "dd-build-server did not become healthy"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

async fn assert_downgrade_rejected(
    client: &reqwest::Client,
    base_url: &str,
    repository: &str,
    request_id: &str,
) {
    let response = client
        .post(format!("{base_url}/builds"))
        .header("x-server-auth", AUTH_SECRET)
        .json(&json!({
            "schemaVersion": "build-server.v1",
            "jobKind": "run-profile",
            "repoUrl": repository,
            "gitRef": REVISION,
            "image": "",
            "profile": "node-verify",
            "requestId": request_id
        }))
        .send()
        .await
        .expect("submit downgrade request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.expect("downgrade response JSON");
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .expect("downgrade rejection message");
    assert!(
        error.contains("not allowed for exact repository identity")
            && error.contains("BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"),
        "unexpected downgrade rejection: {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_repository_profile_downgrades_have_zero_queue_side_effects() {
    let port = reserve_port();
    let base_url = format!("http://127.0.0.1:{port}");
    let work_root = unique_work_root();
    let binary = option_env!("CARGO_BIN_EXE_dd-build-server")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_dd-build-server").map(PathBuf::from))
        .expect("Cargo must expose the dd-build-server binary to integration tests");

    let child = Command::new(binary)
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("RUST_LOG", "error")
        .env("BUILD_SERVER_WORK_ROOT", &work_root)
        .env("BUILD_SERVER_AUTH_SECRET", AUTH_SECRET)
        .env("BUILD_SERVER_NATS_ENABLED", "false")
        .env("BUILD_SERVER_NATS_INTAKE_ENABLED", "false")
        .env("BUILD_SERVER_COORDINATION_ENABLED", "false")
        .env("BUILD_SERVER_GH_SYNC_ENABLED", "false")
        .env("BUILD_SERVER_LAMBDA_ENABLED", "false")
        .env("BUILD_SERVER_DEPLOY_ENABLED", "false")
        .env("BUILD_SERVER_PUSH_ENABLED", "false")
        .env("BUILD_SERVER_MAX_CONCURRENT_BUILDS", "1")
        .env("BUILD_SERVER_MAX_QUEUED", "1")
        .env("BUILD_SERVER_ALLOWED_PROFILES", "rust-verify,node-verify")
        .env(
            "BUILD_SERVER_ALLOWED_REPO_PREFIXES",
            "https://github.com/ORESoftware/,https://github.com/oresoftware/,git@github.com:ORESoftware/,ssh://git@github.com/ORESoftware/",
        )
        .env(
            "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES",
            "https://github.com/ORESoftware/,https://github.com/oresoftware/,git@github.com:ORESoftware/,ssh://git@github.com/ORESoftware/",
        )
        .env(
            "BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON",
            r#"[{"repository":"https://github.com/ORESoftware/k8s-cluster.git","profiles":["rust-verify"]}]"#,
        )
        .env_remove("BUILD_SERVER_DATABASE_URL")
        .env_remove("DATABASE_URL")
        .env_remove("NATS_URL")
        .env_remove("GH_PAT")
        .env_remove("GITHUB_TOKEN")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start dd-build-server");
    let mut child = ChildGuard { child, work_root };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP client");
    wait_until_healthy(&client, &base_url, &mut child).await;

    for (index, repository) in [
        "https://github.com/ORESoftware/k8s-cluster.git",
        "git@github.com:ORESoftware/k8s-cluster.git",
        "https://github.com/oresoftware/K8S-CLUSTER.git/",
    ]
    .into_iter()
    .enumerate()
    {
        assert_downgrade_rejected(
            &client,
            &base_url,
            repository,
            &format!("profile-downgrade-{index}"),
        )
        .await;
    }

    let jobs_response = client
        .get(format!("{base_url}/builds"))
        .header("x-server-auth", AUTH_SECRET)
        .send()
        .await
        .expect("list builds");
    assert_eq!(jobs_response.status(), StatusCode::OK);
    let jobs: Value = jobs_response.json().await.expect("build list JSON");
    assert_eq!(jobs, json!([]), "rejected requests must not create jobs");

    let metrics = client
        .get(format!("{base_url}/metrics"))
        .send()
        .await
        .expect("read metrics")
        .text()
        .await
        .expect("metrics body");
    assert!(metrics.contains("dd_build_server_jobs_submitted_total 0"));
    assert!(metrics.contains("dd_build_server_jobs_queued 0"));
    assert!(metrics.contains("dd_build_server_requests_rejected_total 3"));

    let entries = fs::read_dir(&child.work_root)
        .expect("read build work root")
        .count();
    assert_eq!(entries, 0, "rejected requests must not create workdirs");
}
