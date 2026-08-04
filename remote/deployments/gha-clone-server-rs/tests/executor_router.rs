use std::{
    collections::BTreeMap,
    fs,
    net::TcpListener as StdTcpListener,
    path::PathBuf,
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
const ROUTER_AUTH: &str = "router-test-auth";
const AWS_AUTH: &str = "aws-test-auth";
const HETZNER_AUTH: &str = "hetzner-test-auth";
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
const ROUTER_ENV_VARS: &[&str] = &[
    "HOST",
    "PORT",
    "GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED",
    "GHA_EXECUTOR_ROUTER_AUTH_PATH",
    "GHA_EXECUTOR_ROUTER_EXECUTORS_JSON",
    "GHA_EXECUTOR_ROUTER_MAX_ROUTES",
    "GHA_EXECUTOR_ROUTER_MAX_REQUEST_BYTES",
    "GHA_EXECUTOR_ROUTER_MAX_RESPONSE_BYTES",
    "GHA_EXECUTOR_ROUTER_REQUEST_TIMEOUT_SECONDS",
];

struct SecretFiles {
    directory: PathBuf,
    router: PathBuf,
    aws: PathBuf,
    hetzner: PathBuf,
}

impl SecretFiles {
    fn create() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "gha-executor-router-tests-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&directory).expect("create secret directory");
        let router = directory.join("router-auth");
        let aws = directory.join("aws-auth");
        let hetzner = directory.join("hetzner-auth");
        fs::write(&router, ROUTER_AUTH).expect("write router auth");
        fs::write(&aws, AWS_AUTH).expect("write AWS auth");
        fs::write(&hetzner, HETZNER_AUTH).expect("write Hetzner auth");
        Self {
            directory,
            router,
            aws,
            hetzner,
        }
    }
}

impl Drop for SecretFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

struct RouterProcess {
    child: Child,
    base_url: String,
    _secrets: SecretFiles,
}

impl Drop for RouterProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone)]
struct ExecutorState {
    auth: String,
    build_id: String,
    submit_status: Arc<Mutex<StatusCode>>,
    poll_status: Arc<Mutex<StatusCode>>,
    submissions: Arc<AtomicUsize>,
    polls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
}

struct ExecutorDouble {
    base_url: String,
    state: ExecutorState,
    task: JoinHandle<()>,
}

impl Drop for ExecutorDouble {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn unused_port() -> u16 {
    StdTcpListener::bind(("127.0.0.1", 0))
        .expect("reserve local port")
        .local_addr()
        .expect("local address")
        .port()
}

async fn spawn_executor(
    auth: &str,
    build_id: &str,
    submit_status: StatusCode,
    poll_status: StatusCode,
) -> ExecutorDouble {
    let state = ExecutorState {
        auth: auth.to_string(),
        build_id: build_id.to_string(),
        submit_status: Arc::new(Mutex::new(submit_status)),
        poll_status: Arc::new(Mutex::new(poll_status)),
        submissions: Arc::new(AtomicUsize::new(0)),
        polls: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/builds", post(executor_submit))
        .route("/builds/:id", get(executor_poll))
        .with_state(state.clone());
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind executor double");
    let address = listener.local_addr().expect("executor address");
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("executor double");
    });
    ExecutorDouble {
        base_url: format!("http://{address}"),
        state,
        task,
    }
}

async fn executor_submit(
    State(state): State<ExecutorState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if headers
        .get("x-build-server-auth")
        .and_then(|value| value.to_str().ok())
        != Some(state.auth.as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.submissions.fetch_add(1, Ordering::SeqCst);
    state.requests.lock().await.push(body);
    let status = *state.submit_status.lock().await;
    if status == StatusCode::ACCEPTED {
        (
            status,
            Json(json!({
                "id": state.build_id,
                "status": "queued",
                "error": null
            })),
        )
            .into_response()
    } else {
        (
            status,
            "sensitive-upstream-submission-body-must-not-leak",
        )
            .into_response()
    }
}

async fn executor_poll(
    State(state): State<ExecutorState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if headers
        .get("x-build-server-auth")
        .and_then(|value| value.to_str().ok())
        != Some(state.auth.as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.polls.fetch_add(1, Ordering::SeqCst);
    if id != state.build_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    let status = *state.poll_status.lock().await;
    if status == StatusCode::OK {
        (
            status,
            Json(json!({
                "id": state.build_id,
                "status": "succeeded",
                "error": null
            })),
        )
            .into_response()
    } else {
        (status, "sensitive-upstream-poll-body-must-not-leak").into_response()
    }
}

async fn spawn_router(
    aws_url: &str,
    hetzner_url: &str,
    request_timeout_seconds: u64,
) -> RouterProcess {
    let secrets = SecretFiles::create();
    let executors = json!([
        {
            "id": "aws-primary",
            "provider": "aws",
            "baseUrl": aws_url,
            "authPath": secrets.aws.to_string_lossy()
        },
        {
            "id": "hetzner-secondary",
            "provider": "hetzner",
            "baseUrl": hetzner_url,
            "authPath": secrets.hetzner.to_string_lossy()
        }
    ]);
    let port = unused_port();
    let mut command = Command::new(ROUTER_BINARY);
    for &name in ROUTER_ENV_VARS {
        command.env_remove(name);
    }
    command
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("RUST_LOG", "error")
        .env("GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED", "true")
        .env(
            "GHA_EXECUTOR_ROUTER_AUTH_PATH",
            secrets.router.to_string_lossy().as_ref(),
        )
        .env("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON", executors.to_string())
        .env("GHA_EXECUTOR_ROUTER_MAX_ROUTES", "32")
        .env("GHA_EXECUTOR_ROUTER_MAX_REQUEST_BYTES", "8192")
        .env("GHA_EXECUTOR_ROUTER_MAX_RESPONSE_BYTES", "8192")
        .env(
            "GHA_EXECUTOR_ROUTER_REQUEST_TIMEOUT_SECONDS",
            request_timeout_seconds.to_string(),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn().expect("start executor router");
    let mut router = RouterProcess {
        child,
        base_url: format!("http://127.0.0.1:{port}"),
        _secrets: secrets,
    };
    wait_for_router(&mut router).await;
    router
}

async fn wait_for_router(router: &mut RouterProcess) {
    let client = Client::new();
    for _ in 0..200 {
        if let Some(status) = router.child.try_wait().expect("read router status") {
            panic!("executor router exited before readiness with {status}");
        }
        if let Ok(response) = client
            .get(format!("{}/readyz", router.base_url))
            .send()
            .await
        {
            if response.status() == StatusCode::OK {
                return;
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("executor router did not become ready");
}

fn build_request(request_id: &str) -> Value {
    json!({
        "schemaVersion": "build-server.v1",
        "jobKind": "run-profile",
        "repoUrl": "https://github.com/owner/repo.git",
        "gitRef": REVISION,
        "profile": "rust-verify",
        "requestId": request_id
    })
}

async fn post_build(client: &Client, router: &RouterProcess, request_id: &str) -> ResponseData {
    response_data(
        client
            .post(format!("{}/builds", router.base_url))
            .header("x-build-server-auth", ROUTER_AUTH)
            .json(&build_request(request_id))
            .send()
            .await
            .expect("submit routed build"),
    )
    .await
}

async fn poll_build(client: &Client, router: &RouterProcess, route_id: &str) -> ResponseData {
    response_data(
        client
            .get(format!("{}/builds/{route_id}", router.base_url))
            .header("x-build-server-auth", ROUTER_AUTH)
            .send()
            .await
            .expect("poll routed build"),
    )
    .await
}

struct ResponseData {
    status: StatusCode,
    text: String,
    json: Value,
}

async fn response_data(response: reqwest::Response) -> ResponseData {
    let status = response.status();
    let text = response.text().await.expect("response text");
    let json = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("response was not JSON ({error}): {text}"));
    ResponseData { status, text, json }
}

#[tokio::test]
async fn aws_acceptance_is_idempotent_and_polling_remains_pinned() {
    let aws = spawn_executor(AWS_AUTH, "aws-build-1", StatusCode::ACCEPTED, StatusCode::OK).await;
    let hetzner = spawn_executor(
        HETZNER_AUTH,
        "hetzner-build-1",
        StatusCode::ACCEPTED,
        StatusCode::OK,
    )
    .await;
    let router = spawn_router(&aws.base_url, &hetzner.base_url, 2).await;
    let client = Client::new();

    let first = post_build(&client, &router, "gha-clone:plan:rust").await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    let route_id = first.json["id"].as_str().expect("route id");
    assert!(route_id.starts_with("aws-primary~"));
    assert_eq!(first.json["status"], "queued");

    let duplicate = post_build(&client, &router, "gha-clone:plan:rust").await;
    assert_eq!(duplicate.status, StatusCode::ACCEPTED);
    assert_eq!(duplicate.json["id"], route_id);
    assert_eq!(aws.state.submissions.load(Ordering::SeqCst), 1);
    assert_eq!(hetzner.state.submissions.load(Ordering::SeqCst), 0);
    let requests = aws.state.requests.lock().await;
    assert_eq!(requests[0]["requestId"], "gha-clone:plan:rust");
    drop(requests);

    let terminal = poll_build(&client, &router, route_id).await;
    assert_eq!(terminal.status, StatusCode::OK);
    assert_eq!(terminal.json["id"], route_id);
    assert_eq!(terminal.json["status"], "succeeded");
    assert_eq!(aws.state.polls.load(Ordering::SeqCst), 1);
    assert_eq!(hetzner.state.polls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn aws_5xx_and_429_fail_over_before_acceptance() {
    for status in [StatusCode::SERVICE_UNAVAILABLE, StatusCode::TOO_MANY_REQUESTS] {
        let aws = spawn_executor(AWS_AUTH, "aws-build-2", status, StatusCode::OK).await;
        let hetzner = spawn_executor(
            HETZNER_AUTH,
            "hetzner-build-2",
            StatusCode::ACCEPTED,
            StatusCode::OK,
        )
        .await;
        let router = spawn_router(&aws.base_url, &hetzner.base_url, 2).await;
        let client = Client::new();

        let response = post_build(
            &client,
            &router,
            &format!("gha-clone:plan:{}", status.as_u16()),
        )
        .await;
        assert_eq!(response.status, StatusCode::ACCEPTED);
        assert!(response.json["id"]
            .as_str()
            .expect("route id")
            .starts_with("hetzner-secondary~"));
        assert_eq!(aws.state.submissions.load(Ordering::SeqCst), 1);
        assert_eq!(hetzner.state.submissions.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn connection_refusal_fails_over_but_contract_rejection_does_not() {
    let unreachable = format!("http://127.0.0.1:{}", unused_port());
    let hetzner = spawn_executor(
        HETZNER_AUTH,
        "hetzner-build-3",
        StatusCode::ACCEPTED,
        StatusCode::OK,
    )
    .await;
    let router = spawn_router(&unreachable, &hetzner.base_url, 1).await;
    let client = Client::new();
    let response = post_build(&client, &router, "gha-clone:connect-failover").await;
    assert_eq!(response.status, StatusCode::ACCEPTED);
    assert!(response.json["id"]
        .as_str()
        .expect("route id")
        .starts_with("hetzner-secondary~"));
    assert_eq!(hetzner.state.submissions.load(Ordering::SeqCst), 1);

    let aws = spawn_executor(AWS_AUTH, "aws-build-4", StatusCode::BAD_REQUEST, StatusCode::OK).await;
    let untouched_hetzner = spawn_executor(
        HETZNER_AUTH,
        "hetzner-build-4",
        StatusCode::ACCEPTED,
        StatusCode::OK,
    )
    .await;
    let router = spawn_router(&aws.base_url, &untouched_hetzner.base_url, 2).await;
    let rejected = post_build(&client, &router, "gha-clone:contract-rejection").await;
    assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!rejected
        .text
        .contains("sensitive-upstream-submission-body-must-not-leak"));
    assert_eq!(aws.state.submissions.load(Ordering::SeqCst), 1);
    assert_eq!(untouched_hetzner.state.submissions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn accepted_aws_poll_failure_is_never_resubmitted_to_hetzner() {
    let aws = spawn_executor(AWS_AUTH, "aws-build-5", StatusCode::ACCEPTED, StatusCode::OK).await;
    let hetzner = spawn_executor(
        HETZNER_AUTH,
        "hetzner-build-5",
        StatusCode::ACCEPTED,
        StatusCode::OK,
    )
    .await;
    let router = spawn_router(&aws.base_url, &hetzner.base_url, 2).await;
    let client = Client::new();
    let accepted = post_build(&client, &router, "gha-clone:pinned-poll").await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED);
    let route_id = accepted.json["id"].as_str().expect("route id");
    *aws.state.poll_status.lock().await = StatusCode::SERVICE_UNAVAILABLE;

    let failed_poll = poll_build(&client, &router, route_id).await;
    assert_eq!(failed_poll.status, StatusCode::BAD_GATEWAY);
    assert!(!failed_poll
        .text
        .contains("sensitive-upstream-poll-body-must-not-leak"));
    assert!(failed_poll.text.contains("not resubmitted"));
    assert_eq!(aws.state.submissions.load(Ordering::SeqCst), 1);
    assert_eq!(hetzner.state.submissions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn health_capabilities_metrics_and_auth_are_source_redacted() {
    let aws = spawn_executor(AWS_AUTH, "aws-build-6", StatusCode::ACCEPTED, StatusCode::OK).await;
    let hetzner = spawn_executor(
        HETZNER_AUTH,
        "hetzner-build-6",
        StatusCode::ACCEPTED,
        StatusCode::OK,
    )
    .await;
    let router = spawn_router(&aws.base_url, &hetzner.base_url, 2).await;
    let client = Client::new();

    for path in ["/", "/healthz", "/readyz", "/v1/capabilities", "/metrics"] {
        let response = client
            .get(format!("{}{path}", router.base_url))
            .send()
            .await
            .expect("public endpoint");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.text().await.expect("public body");
        assert!(!body.contains(ROUTER_AUTH));
        assert!(!body.contains(AWS_AUTH));
        assert!(!body.contains(HETZNER_AUTH));
        assert!(!body.contains(&aws.base_url));
        assert!(!body.contains(&hetzner.base_url));
    }

    let unauthorized = client
        .post(format!("{}/builds", router.base_url))
        .header("x-build-server-auth", "wrong")
        .json(&build_request("gha-clone:unauthorized"))
        .send()
        .await
        .expect("unauthorized request");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(aws.state.submissions.load(Ordering::SeqCst), 0);
    assert_eq!(hetzner.state.submissions.load(Ordering::SeqCst), 0);
}

#[test]
fn router_environment_contract_is_unique() {
    let unique = ROUTER_ENV_VARS.iter().copied().collect::<BTreeMap<_, _>>();
    let _ = unique;
    let mut sorted = ROUTER_ENV_VARS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ROUTER_ENV_VARS.len());
}
