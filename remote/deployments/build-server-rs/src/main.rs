use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    process::Command,
    sync::{RwLock, Semaphore},
    time::timeout,
};

const SERVICE_NAME: &str = "dd-build-server";
const DEFAULT_PORT: u16 = 8100;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    http: reqwest::Client,
    jobs: Arc<RwLock<HashMap<String, BuildJobRecord>>>,
    semaphore: Arc<Semaphore>,
    counters: Arc<Counters>,
}

#[derive(Clone)]
struct Config {
    work_root: PathBuf,
    git_bin: String,
    nerdctl_bin: String,
    kubectl_bin: String,
    containerd_namespace: String,
    allowed_repo_prefixes: Vec<String>,
    allowed_image_prefixes: Vec<String>,
    allowed_namespaces: HashSet<String>,
    deploy_enabled: bool,
    push_enabled: bool,
    ecr_login_enabled: bool,
    aws_region: String,
    job_timeout: Duration,
    max_log_bytes: u64,
    max_jobs: usize,
    server_auth_secret: Option<String>,
}

#[derive(Default)]
struct Counters {
    submitted: AtomicU64,
    running: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    rejected: AtomicU64,
    command_failures: AtomicU64,
    ecr_logins: AtomicU64,
    ecr_login_failures: AtomicU64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildRequest {
    schema_version: Option<String>,
    job_kind: Option<String>,
    repo_url: String,
    git_ref: Option<String>,
    image: String,
    context_dir: Option<String>,
    dockerfile: Option<String>,
    build_args: Option<BTreeMap<String, String>>,
    push: Option<bool>,
    deploy: Option<DeployRequest>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeployRequest {
    kind: String,
    path: String,
    namespace: Option<String>,
    rollout: Option<String>,
    rollout_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildJobRecord {
    id: String,
    status: BuildStatus,
    request: BuildRequest,
    created_at_ms: u128,
    started_at_ms: Option<u128>,
    finished_at_ms: Option<u128>,
    log_path: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
enum BuildStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    auth_configured: bool,
    deploy_enabled: bool,
    push_enabled: bool,
    ecr_login_enabled: bool,
    allowed_repo_prefixes: Vec<String>,
    allowed_image_prefixes: Vec<String>,
    allowed_namespaces: Vec<String>,
    queued: usize,
    running: u64,
}

#[derive(Debug, Clone)]
struct EcrImage {
    registry: String,
    region: String,
}

#[derive(Debug)]
struct AwsCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EcrAuthResponse {
    authorization_data: Vec<EcrAuthorizationData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EcrAuthorizationData {
    authorization_token: String,
    proxy_endpoint: String,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    first_env(&[key])
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_usize(key: &str, fallback: usize) -> usize {
    first_env(&[key])
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn parse_namespaces(value: &str) -> HashSet<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn config_from_env() -> Config {
    Config {
        work_root: PathBuf::from(env_value(
            "BUILD_SERVER_WORK_ROOT",
            "/var/lib/dd-build-server/jobs",
        )),
        git_bin: env_value("BUILD_SERVER_GIT_BIN", "git"),
        nerdctl_bin: env_value("BUILD_SERVER_NERDCTL_BIN", "/usr/local/bin/nerdctl"),
        kubectl_bin: env_value("BUILD_SERVER_KUBECTL_BIN", "/usr/bin/kubectl"),
        containerd_namespace: env_value("BUILD_SERVER_CONTAINERD_NAMESPACE", "k8s.io"),
        allowed_repo_prefixes: parse_csv(&env_value("BUILD_SERVER_ALLOWED_REPO_PREFIXES", "")),
        allowed_image_prefixes: parse_csv(&env_value("BUILD_SERVER_ALLOWED_IMAGE_PREFIXES", "")),
        allowed_namespaces: parse_namespaces(&env_value(
            "BUILD_SERVER_ALLOWED_NAMESPACES",
            "default",
        )),
        deploy_enabled: env_bool("BUILD_SERVER_DEPLOY_ENABLED", true),
        push_enabled: env_bool("BUILD_SERVER_PUSH_ENABLED", false),
        ecr_login_enabled: env_bool("BUILD_SERVER_ECR_LOGIN_ENABLED", true),
        aws_region: first_env(&["AWS_REGION", "AWS_DEFAULT_REGION"])
            .unwrap_or_else(|| "us-east-1".to_string()),
        job_timeout: Duration::from_secs(env_u64("BUILD_SERVER_JOB_TIMEOUT_SECONDS", 1_800)),
        max_log_bytes: env_u64("BUILD_SERVER_MAX_LOG_BYTES", 4 * 1024 * 1024),
        max_jobs: env_usize("BUILD_SERVER_MAX_JOBS", 200),
        server_auth_secret: first_env(&["BUILD_SERVER_AUTH_SECRET", "SERVER_AUTH_SECRET"]),
    }
}

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-build-server-auth"))
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == secret)
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "SERVER_AUTH_SECRET is not configured" })),
        )
            .into_response());
    };
    if !request_is_authorized(headers, secret) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "errMessage": "missing required build server auth header",
            })),
        )
            .into_response());
    }
    Ok(())
}

fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn ensure_allowed_prefix(
    name: &str,
    value: &str,
    prefixes: &[String],
    env_name: &str,
) -> Result<(), String> {
    if prefixes.is_empty() || prefixes.iter().any(|prefix| value.starts_with(prefix)) {
        Ok(())
    } else {
        Err(format!("{name} is not allowed by {env_name}"))
    }
}

fn validate_no_whitespace(name: &str, value: &str, max_len: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if value.len() > max_len {
        return Err(format!("{name} must be {max_len} characters or fewer"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{name} must not contain whitespace"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{name} must not contain control characters"));
    }
    Ok(())
}

fn validate_repo_url(repo_url: &str) -> Result<(), String> {
    let repo_url = repo_url.trim();
    if repo_url.is_empty() {
        return Err("repoUrl is required".to_string());
    }
    if repo_url.len() > 2048 {
        return Err("repoUrl must be 2048 characters or fewer".to_string());
    }
    if repo_url.chars().any(char::is_control) {
        return Err("repoUrl must not contain control characters".to_string());
    }
    if repo_url.starts_with("https://")
        || repo_url.starts_with("ssh://")
        || repo_url.starts_with("git@")
    {
        Ok(())
    } else {
        Err("repoUrl must use https://, ssh://, or git@".to_string())
    }
}

fn has_explicit_image_version(image: &str) -> bool {
    let last_path = image.rsplit('/').next().unwrap_or(image);
    image.contains('@') || last_path.contains(':')
}

fn image_registry(image: &str) -> Option<&str> {
    let first = image.split('/').next().unwrap_or_default();
    if first.contains('.') || first.contains(':') || first == "localhost" {
        Some(first)
    } else {
        None
    }
}

fn ecr_image(image: &str) -> Option<EcrImage> {
    let registry = image_registry(image)?;
    let parts = registry.split('.').collect::<Vec<_>>();
    if parts.len() >= 6
        && parts[1] == "dkr"
        && parts[2] == "ecr"
        && parts[4] == "amazonaws"
        && (parts[5] == "com" || parts[5] == "com.cn")
    {
        return Some(EcrImage {
            registry: registry.to_string(),
            region: parts[3].to_string(),
        });
    }
    None
}

fn validate_image(config: &Config, image: &str, push: bool) -> Result<Option<EcrImage>, String> {
    validate_no_whitespace("image", image, 512)?;
    if !has_explicit_image_version(image) {
        return Err("image must include an explicit tag or digest".to_string());
    }
    ensure_allowed_prefix(
        "image",
        image,
        &config.allowed_image_prefixes,
        "BUILD_SERVER_ALLOWED_IMAGE_PREFIXES",
    )?;
    let ecr = ecr_image(image);
    if push && config.ecr_login_enabled && ecr.is_none() {
        return Err(
            "push currently requires an Amazon ECR image when ECR login is enabled".to_string(),
        );
    }
    Ok(ecr)
}

fn validate_relative_path(name: &str, value: &str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if trimmed.len() > 240 {
        return Err(format!("{name} must be 240 characters or fewer"));
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(format!("{name} must be relative to the repository root"));
    }

    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{name} must stay inside the repository root"));
            }
        }
    }

    if clean.as_os_str().is_empty() {
        clean.push(".");
    }
    Ok(clean)
}

fn validate_build_args(build_args: &Option<BTreeMap<String, String>>) -> Result<(), String> {
    let Some(build_args) = build_args else {
        return Ok(());
    };
    if build_args.len() > 32 {
        return Err("buildArgs can contain at most 32 entries".to_string());
    }
    for (key, value) in build_args {
        if key.is_empty() || key.len() > 80 {
            return Err("build arg keys must be 1-80 characters".to_string());
        }
        let upper_key = key.to_ascii_uppercase();
        if ["SECRET", "PASSWORD", "TOKEN", "CREDENTIAL", "PRIVATE_KEY"]
            .iter()
            .any(|part| upper_key.contains(part))
        {
            return Err(format!(
                "build arg key {key:?} looks secret-like; use registry/repo credentials, not Docker build args"
            ));
        }
        if !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return Err(format!(
                "build arg key {key:?} contains unsupported characters"
            ));
        }
        if value.len() > 1024 || value.chars().any(char::is_control) {
            return Err(format!(
                "build arg {key:?} must be printable and 1024 characters or fewer"
            ));
        }
    }
    Ok(())
}

fn validate_namespace(config: &Config, namespace: &str) -> Result<(), String> {
    validate_no_whitespace("deploy.namespace", namespace, 63)?;
    if !config.allowed_namespaces.contains(namespace) {
        return Err(format!(
            "namespace {namespace:?} is not allowed by BUILD_SERVER_ALLOWED_NAMESPACES"
        ));
    }
    Ok(())
}

fn validate_rollout_resource(value: &str) -> Result<String, String> {
    let value = value.trim();
    validate_no_whitespace("deploy.rollout", value, 160)?;
    if value.contains("..") {
        return Err("deploy.rollout must not contain '..'".to_string());
    }
    if value.contains('/') {
        Ok(value.to_string())
    } else {
        Ok(format!("deployment/{value}"))
    }
}

fn validate_deploy(config: &Config, deploy: &Option<DeployRequest>) -> Result<(), String> {
    let Some(deploy) = deploy else {
        return Ok(());
    };
    match deploy.kind.as_str() {
        "kustomize" | "manifest" | "none" => {}
        _ => return Err("deploy.kind must be one of: kustomize, manifest, none".to_string()),
    }
    if deploy.kind == "none" {
        return Ok(());
    }
    if !config.deploy_enabled {
        return Err("deploy is disabled by BUILD_SERVER_DEPLOY_ENABLED=false".to_string());
    }
    validate_relative_path("deploy.path", &deploy.path)?;
    let namespace = deploy.namespace.as_deref().unwrap_or("default");
    validate_namespace(config, namespace)?;
    if let Some(rollout) = deploy.rollout.as_deref() {
        validate_rollout_resource(rollout)?;
    }
    Ok(())
}

fn validate_build_request(config: &Config, request: &BuildRequest) -> Result<(), String> {
    if let Some(schema_version) = clean_optional(request.schema_version.as_deref()) {
        if schema_version != "build-server.v1" {
            return Err("schemaVersion must be build-server.v1".to_string());
        }
    }
    if let Some(job_kind) = clean_optional(request.job_kind.as_deref()) {
        if !matches!(job_kind.as_str(), "build-image" | "build-and-deploy") {
            return Err("jobKind must be build-image or build-and-deploy".to_string());
        }
    }
    validate_repo_url(&request.repo_url)?;
    ensure_allowed_prefix(
        "repoUrl",
        &request.repo_url,
        &config.allowed_repo_prefixes,
        "BUILD_SERVER_ALLOWED_REPO_PREFIXES",
    )?;
    validate_image(config, &request.image, request.push.unwrap_or(false))?;
    if let Some(git_ref) = clean_optional(request.git_ref.as_deref()) {
        validate_no_whitespace("gitRef", &git_ref, 180)?;
    }
    validate_relative_path("contextDir", request.context_dir.as_deref().unwrap_or("."))?;
    validate_relative_path(
        "dockerfile",
        request.dockerfile.as_deref().unwrap_or("Dockerfile"),
    )?;
    validate_build_args(&request.build_args)?;
    if request.push.unwrap_or(false) && !config.push_enabled {
        return Err("push is disabled by BUILD_SERVER_PUSH_ENABLED=false".to_string());
    }
    validate_deploy(config, &request.deploy)
}

fn shellish(value: &str) -> String {
    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '=' | '@')
    }) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn printable_command(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .map(|value| shellish(&value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn redacted_build_args(args: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            let key = arg.split_once('=').map(|(key, _)| key).unwrap_or(arg);
            redacted.push(format!("{key}=<redacted>"));
            redact_next = false;
            continue;
        }
        redacted.push(arg.clone());
        if arg == "--build-arg" {
            redact_next = true;
        }
    }
    redacted
}

async fn append_log(path: &Path, message: &str, max_bytes: u64) {
    let current_len = fs::metadata(path).await.map(|meta| meta.len()).unwrap_or(0);
    if current_len >= max_bytes {
        return;
    }
    let remaining = (max_bytes - current_len) as usize;
    let bytes = message.as_bytes();
    let limit = remaining.min(bytes.len());
    if limit == 0 {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        let _ = file.write_all(&bytes[..limit]).await;
    }
}

async fn pipe_reader<R>(reader: R, log_path: PathBuf, prefix: &'static str, max_bytes: u64)
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&line);
                append_log(&log_path, &format!("{prefix}{text}"), max_bytes).await;
            }
            Err(error) => {
                append_log(
                    &log_path,
                    &format!("{prefix}failed to read command output: {error}\n"),
                    max_bytes,
                )
                .await;
                break;
            }
        }
    }
}

async fn run_logged_command(
    config: &Config,
    log_path: &Path,
    cwd: &Path,
    program: &str,
    args: Vec<String>,
) -> Result<(), String> {
    run_logged_command_inner(config, log_path, cwd, program, args, None, None).await
}

async fn run_logged_command_with_input(
    config: &Config,
    log_path: &Path,
    cwd: &Path,
    program: &str,
    args: Vec<String>,
    display_args: Vec<String>,
    stdin: Vec<u8>,
) -> Result<(), String> {
    run_logged_command_inner(
        config,
        log_path,
        cwd,
        program,
        args,
        Some(display_args),
        Some(stdin),
    )
    .await
}

async fn run_logged_command_inner(
    config: &Config,
    log_path: &Path,
    cwd: &Path,
    program: &str,
    args: Vec<String>,
    display_args: Option<Vec<String>>,
    stdin: Option<Vec<u8>>,
) -> Result<(), String> {
    let display_args = display_args.unwrap_or_else(|| args.clone());
    append_log(
        log_path,
        &format!("\n$ {}\n", printable_command(program, &display_args)),
        config.max_log_bytes,
    )
    .await;

    let mut command = Command::new(program);
    command
        .args(&args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", cwd)
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    for key in [
        "KUBERNETES_SERVICE_HOST",
        "KUBERNETES_SERVICE_PORT",
        "KUBERNETES_SERVICE_PORT_HTTPS",
    ] {
        if let Ok(value) = env::var(key) {
            command.env(key, value);
        }
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn {program}: {error}"))?;
    if let Some(input) = stdin {
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin
                .write_all(&input)
                .await
                .map_err(|error| format!("failed to write stdin for {program}: {error}"))?;
        }
    }

    let stdout_task = child.stdout.take().map(|stdout| {
        tokio::spawn(pipe_reader(
            stdout,
            log_path.to_path_buf(),
            "",
            config.max_log_bytes,
        ))
    });
    let stderr_task = child.stderr.take().map(|stderr| {
        tokio::spawn(pipe_reader(
            stderr,
            log_path.to_path_buf(),
            "",
            config.max_log_bytes,
        ))
    });

    let status = match timeout(config.job_timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => return Err(format!("{program} failed to wait: {error}")),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(format!(
                "{program} timed out after {:?}",
                config.job_timeout
            ));
        }
    };

    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }

    append_log(
        log_path,
        &format!("exit status: {status}\n"),
        config.max_log_bytes,
    )
    .await;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

type HmacSha256 = Hmac<Sha256>;

fn hmac_sha256(key: &[u8], value: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any size");
    mac.update(value.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn aws_timestamp() -> (String, String) {
    let now = OffsetDateTime::now_utc();
    let date = format!(
        "{:04}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    );
    let timestamp = format!(
        "{}T{:02}{:02}{:02}Z",
        date,
        now.hour(),
        now.minute(),
        now.second()
    );
    (date, timestamp)
}

fn aws_credentials_from_env() -> Result<AwsCredentials, String> {
    let access_key_id = first_env(&["AWS_ACCESS_KEY_ID"])
        .ok_or_else(|| "AWS_ACCESS_KEY_ID is required for ECR push".to_string())?;
    let secret_access_key = first_env(&["AWS_SECRET_ACCESS_KEY"])
        .ok_or_else(|| "AWS_SECRET_ACCESS_KEY is required for ECR push".to_string())?;
    let session_token = first_env(&["AWS_SESSION_TOKEN"]);
    Ok(AwsCredentials {
        access_key_id,
        secret_access_key,
        session_token,
    })
}

fn ecr_headers(
    config: &Config,
    credentials: &AwsCredentials,
    region: &str,
    host: &str,
    body: &str,
) -> Result<ReqwestHeaderMap, String> {
    let target = "AmazonEC2ContainerRegistry_V20150921.GetAuthorizationToken";
    let content_type = "application/x-amz-json-1.1";
    let (date, timestamp) = aws_timestamp();
    let session_token = credentials.session_token.as_deref().unwrap_or("");
    let (canonical_headers, signed_headers) = if session_token.is_empty() {
        (
            format!("content-type:{content_type}\nhost:{host}\nx-amz-date:{timestamp}\nx-amz-target:{target}\n"),
            "content-type;host;x-amz-date;x-amz-target",
        )
    } else {
        (
            format!(
                "content-type:{content_type}\nhost:{host}\nx-amz-date:{timestamp}\nx-amz-security-token:{session_token}\nx-amz-target:{target}\n"
            ),
            "content-type;host;x-amz-date;x-amz-security-token;x-amz-target",
        )
    };
    let canonical_request = format!(
        "POST\n/\n\n{canonical_headers}\n{signed_headers}\n{}",
        sha256_hex(body)
    );
    let credential_scope = format!("{date}/{region}/ecr/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{credential_scope}\n{}",
        sha256_hex(&canonical_request)
    );
    let date_key = hmac_sha256(
        format!("AWS4{}", credentials.secret_access_key).as_bytes(),
        &date,
    );
    let region_key = hmac_sha256(&date_key, region);
    let service_key = hmac_sha256(&region_key, "ecr");
    let signing_key = hmac_sha256(&service_key, "aws4_request");
    let signature = hex::encode(hmac_sha256(&signing_key, &string_to_sign));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key_id
    );

    let mut headers = ReqwestHeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static(content_type));
    headers.insert(
        "x-amz-date",
        HeaderValue::from_str(&timestamp).map_err(|error| error.to_string())?,
    );
    headers.insert("x-amz-target", HeaderValue::from_static(target));
    headers.insert(
        "authorization",
        HeaderValue::from_str(&authorization).map_err(|error| error.to_string())?,
    );
    if !session_token.is_empty() {
        headers.insert(
            "x-amz-security-token",
            HeaderValue::from_str(session_token).map_err(|error| error.to_string())?,
        );
    }
    headers.insert(
        "user-agent",
        HeaderValue::from_str(&format!("{SERVICE_NAME}/0.1 ({})", config.aws_region))
            .map_err(|error| error.to_string())?,
    );
    Ok(headers)
}

async fn ecr_authorization_password(state: &AppState, ecr: &EcrImage) -> Result<String, String> {
    let credentials = aws_credentials_from_env()?;
    let body = "{}";
    let host = format!("api.ecr.{}.amazonaws.com", ecr.region);
    let headers = ecr_headers(&state.config, &credentials, &ecr.region, &host, body)?;
    let response = state
        .http
        .post(format!("https://{host}/"))
        .headers(headers)
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| format!("failed to request ECR authorization token: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("failed to read ECR authorization response: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "ECR authorization failed with HTTP {}: {}",
            status.as_u16(),
            text.chars().take(400).collect::<String>()
        ));
    }
    let parsed: EcrAuthResponse = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse ECR authorization response: {error}"))?;
    let data = parsed
        .authorization_data
        .iter()
        .find(|data| data.proxy_endpoint.trim_start_matches("https://") == ecr.registry)
        .or_else(|| parsed.authorization_data.first())
        .ok_or_else(|| {
            "ECR authorization response did not include authorizationData".to_string()
        })?;
    let decoded = BASE64
        .decode(&data.authorization_token)
        .map_err(|error| format!("failed to decode ECR authorization token: {error}"))?;
    let decoded = String::from_utf8(decoded)
        .map_err(|error| format!("ECR authorization token was not UTF-8: {error}"))?;
    let Some((username, password)) = decoded.split_once(':') else {
        return Err("ECR authorization token did not contain username/password".to_string());
    };
    if username != "AWS" {
        return Err("ECR authorization token had an unexpected username".to_string());
    }
    Ok(password.to_string())
}

async fn login_to_ecr(
    state: &AppState,
    log_path: &Path,
    cwd: &Path,
    ecr: &EcrImage,
) -> Result<(), String> {
    append_log(
        log_path,
        &format!("requesting ECR login token for {}\n", ecr.registry),
        state.config.max_log_bytes,
    )
    .await;
    let password = ecr_authorization_password(state, ecr).await?;
    let args = vec![
        "-n".to_string(),
        state.config.containerd_namespace.clone(),
        "login".to_string(),
        "--username".to_string(),
        "AWS".to_string(),
        "--password-stdin".to_string(),
        ecr.registry.clone(),
    ];
    let display_args = args.clone();
    run_logged_command_with_input(
        &state.config,
        log_path,
        cwd,
        &state.config.nerdctl_bin,
        args,
        display_args,
        format!("{password}\n").into_bytes(),
    )
    .await
}

fn job_id(counter: u64) -> String {
    format!("build-{}-{counter}", now_ms())
}

async fn update_job<F>(state: &AppState, id: &str, mutate: F)
where
    F: FnOnce(&mut BuildJobRecord),
{
    let mut jobs = state.jobs.write().await;
    if let Some(job) = jobs.get_mut(id) {
        mutate(job);
    }
}

async fn prune_jobs(state: &AppState) {
    let max_jobs = state.config.max_jobs;
    let mut jobs = state.jobs.write().await;
    if jobs.len() <= max_jobs {
        return;
    }

    let mut candidates = jobs
        .values()
        .filter(|job| !matches!(job.status, BuildStatus::Queued | BuildStatus::Running))
        .map(|job| (job.created_at_ms, job.id.clone()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(created_at_ms, _)| *created_at_ms);
    for (_, id) in candidates
        .into_iter()
        .take(jobs.len().saturating_sub(max_jobs))
    {
        jobs.remove(&id);
    }
}

fn resolve_repo_path(repo_dir: &Path, name: &str, value: &str) -> Result<PathBuf, String> {
    let clean = validate_relative_path(name, value)?;
    Ok(repo_dir.join(clean))
}

async fn execute_build(state: &AppState, job: &BuildJobRecord) -> Result<(), String> {
    let config = state.config.as_ref();
    let request = &job.request;
    let job_dir = config.work_root.join(&job.id);
    let repo_dir = job_dir.join("repo");
    let log_path = PathBuf::from(&job.log_path);

    fs::create_dir_all(&job_dir)
        .await
        .map_err(|error| format!("failed to create job dir: {error}"))?;
    append_log(
        &log_path,
        &format!(
            "{SERVICE_NAME} starting job={} repo={} image={}\n",
            job.id, request.repo_url, request.image
        ),
        config.max_log_bytes,
    )
    .await;

    let mut clone_args = vec!["clone".to_string(), "--depth".to_string(), "1".to_string()];
    if let Some(git_ref) = clean_optional(request.git_ref.as_deref()) {
        clone_args.push("--branch".to_string());
        clone_args.push(git_ref);
    }
    clone_args.push(request.repo_url.clone());
    clone_args.push(repo_dir.to_string_lossy().to_string());
    run_logged_command(config, &log_path, &job_dir, &config.git_bin, clone_args).await?;

    let context_path = resolve_repo_path(
        &repo_dir,
        "contextDir",
        request.context_dir.as_deref().unwrap_or("."),
    )?;
    let dockerfile_path = resolve_repo_path(
        &repo_dir,
        "dockerfile",
        request.dockerfile.as_deref().unwrap_or("Dockerfile"),
    )?;

    let mut build_args = vec![
        "-n".to_string(),
        config.containerd_namespace.clone(),
        "build".to_string(),
        "-f".to_string(),
        dockerfile_path.to_string_lossy().to_string(),
        "-t".to_string(),
        request.image.clone(),
    ];
    if let Some(args) = &request.build_args {
        for (key, value) in args {
            build_args.push("--build-arg".to_string());
            build_args.push(format!("{key}={value}"));
        }
    }
    build_args.push(context_path.to_string_lossy().to_string());
    let display_build_args = redacted_build_args(&build_args);
    run_logged_command_inner(
        config,
        &log_path,
        &repo_dir,
        &config.nerdctl_bin,
        build_args,
        Some(display_build_args),
        None,
    )
    .await?;

    if request.push.unwrap_or(false) {
        let ecr = validate_image(config, &request.image, true)?;
        if config.ecr_login_enabled {
            let ecr = ecr.ok_or_else(|| "push requires an ECR image".to_string())?;
            match login_to_ecr(state, &log_path, &repo_dir, &ecr).await {
                Ok(()) => {
                    state.counters.ecr_logins.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    state
                        .counters
                        .ecr_login_failures
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        run_logged_command(
            config,
            &log_path,
            &repo_dir,
            &config.nerdctl_bin,
            vec![
                "-n".to_string(),
                config.containerd_namespace.clone(),
                "push".to_string(),
                request.image.clone(),
            ],
        )
        .await?;
    }

    if let Some(deploy) = &request.deploy {
        if deploy.kind != "none" {
            let namespace = deploy.namespace.as_deref().unwrap_or("default");
            let deploy_path = resolve_repo_path(&repo_dir, "deploy.path", &deploy.path)?;
            let mut apply_args = vec!["-n".to_string(), namespace.to_string(), "apply".to_string()];
            match deploy.kind.as_str() {
                "kustomize" => {
                    apply_args.push("-k".to_string());
                    apply_args.push(deploy_path.to_string_lossy().to_string());
                }
                "manifest" => {
                    apply_args.push("-f".to_string());
                    apply_args.push(deploy_path.to_string_lossy().to_string());
                }
                _ => unreachable!("deploy kind is validated before queueing"),
            }
            run_logged_command(
                config,
                &log_path,
                &repo_dir,
                &config.kubectl_bin,
                apply_args,
            )
            .await?;

            if let Some(rollout) = deploy.rollout.as_deref() {
                let resource = validate_rollout_resource(rollout)?;
                let timeout_seconds = deploy.rollout_timeout_seconds.unwrap_or(300);
                run_logged_command(
                    config,
                    &log_path,
                    &repo_dir,
                    &config.kubectl_bin,
                    vec![
                        "-n".to_string(),
                        namespace.to_string(),
                        "rollout".to_string(),
                        "status".to_string(),
                        resource,
                        format!("--timeout={timeout_seconds}s"),
                    ],
                )
                .await?;
            }
        }
    }

    append_log(
        &log_path,
        &format!("{SERVICE_NAME} completed job={}\n", job.id),
        config.max_log_bytes,
    )
    .await;
    Ok(())
}

async fn run_job(state: AppState, id: String) {
    let permit = match state.semaphore.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(error) => {
            update_job(&state, &id, |job| {
                job.status = BuildStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(format!("build queue is closed: {error}"));
            })
            .await;
            return;
        }
    };

    state.counters.running.fetch_add(1, Ordering::Relaxed);
    update_job(&state, &id, |job| {
        job.status = BuildStatus::Running;
        job.started_at_ms = Some(now_ms());
    })
    .await;

    let job = {
        let jobs = state.jobs.read().await;
        jobs.get(&id).cloned()
    };

    let result = match job {
        Some(job) => execute_build(&state, &job).await,
        None => Err("job disappeared before execution".to_string()),
    };

    state.counters.running.fetch_sub(1, Ordering::Relaxed);
    drop(permit);

    match result {
        Ok(()) => {
            state.counters.succeeded.fetch_add(1, Ordering::Relaxed);
            update_job(&state, &id, |job| {
                job.status = BuildStatus::Succeeded;
                job.finished_at_ms = Some(now_ms());
                job.error = None;
            })
            .await;
        }
        Err(error) => {
            state.counters.failed.fetch_add(1, Ordering::Relaxed);
            state
                .counters
                .command_failures
                .fetch_add(1, Ordering::Relaxed);
            update_job(&state, &id, |job| {
                job.status = BuildStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(error);
            })
            .await;
        }
    }
}

async fn descriptor() -> impl IntoResponse {
    Json(json!({
        "service": SERVICE_NAME,
        "description": "Authenticated Rust build server for repo image builds and controlled Kubernetes deploys.",
        "endpoints": {
            "submit": "POST /builds",
            "list": "GET /builds",
            "status": "GET /builds/<jobId>",
            "logs": "GET /builds/<jobId>/logs",
            "healthz": "GET /healthz",
            "metrics": "GET /metrics"
        },
        "jobSchema": {
            "schemaVersion": "build-server.v1",
            "jobKind": ["build-image", "build-and-deploy"],
            "required": ["repoUrl", "image"],
            "optional": ["gitRef", "contextDir", "dockerfile", "buildArgs", "push", "deploy"]
        },
        "pushRegistries": ["amazon-ecr"],
        "deployKinds": ["kustomize", "manifest", "none"]
    }))
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, BuildStatus::Queued))
        .count();
    let mut allowed_namespaces = state
        .config
        .allowed_namespaces
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    allowed_namespaces.sort();
    let mut allowed_repo_prefixes = state.config.allowed_repo_prefixes.clone();
    allowed_repo_prefixes.sort();
    let mut allowed_image_prefixes = state.config.allowed_image_prefixes.clone();
    allowed_image_prefixes.sort();

    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        auth_configured: state.config.server_auth_secret.is_some(),
        deploy_enabled: state.config.deploy_enabled,
        push_enabled: state.config.push_enabled,
        ecr_login_enabled: state.config.ecr_login_enabled,
        allowed_repo_prefixes,
        allowed_image_prefixes,
        allowed_namespaces,
        queued,
        running: state.counters.running.load(Ordering::Relaxed),
    })
}

fn build_dependencies_ready(config: &Config) -> bool {
    config.server_auth_secret.is_some()
        && config.work_root.exists()
        && executable_available(&config.git_bin)
        && executable_available(&config.nerdctl_bin)
        && (!config.deploy_enabled || executable_available(&config.kubectl_bin))
}

fn executable_available(value: &str) -> bool {
    let path = Path::new(value);
    if path.is_absolute() || path.components().count() > 1 {
        return path.is_file();
    }
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths)
            .map(|directory| directory.join(value))
            .any(|candidate| candidate.is_file())
    })
}

async fn readyz(State(state): State<AppState>) -> Response {
    let ready = build_dependencies_ready(&state.config);
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "dependenciesReady": ready,
        })),
    )
        .into_response()
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, BuildStatus::Queued))
        .count();
    let mut body = format!(
        "# HELP dd_build_server_jobs_submitted_total Build jobs accepted by the build server.\n\
         # TYPE dd_build_server_jobs_submitted_total counter\n\
         dd_build_server_jobs_submitted_total {}\n\
         # HELP dd_build_server_jobs_running Current running build jobs.\n\
         # TYPE dd_build_server_jobs_running gauge\n\
         dd_build_server_jobs_running {}\n\
         # HELP dd_build_server_jobs_queued Current queued build jobs.\n\
         # TYPE dd_build_server_jobs_queued gauge\n\
         dd_build_server_jobs_queued {}\n\
         # HELP dd_build_server_jobs_succeeded_total Build jobs that completed successfully.\n\
         # TYPE dd_build_server_jobs_succeeded_total counter\n\
         dd_build_server_jobs_succeeded_total {}\n\
         # HELP dd_build_server_jobs_failed_total Build jobs that failed.\n\
         # TYPE dd_build_server_jobs_failed_total counter\n\
         dd_build_server_jobs_failed_total {}\n\
         # HELP dd_build_server_requests_rejected_total Requests rejected before queueing.\n\
         # TYPE dd_build_server_requests_rejected_total counter\n\
         dd_build_server_requests_rejected_total {}\n\
         # HELP dd_build_server_command_failures_total Build pipeline command failures.\n\
         # TYPE dd_build_server_command_failures_total counter\n\
         dd_build_server_command_failures_total {}\n\
         # HELP dd_build_server_ecr_logins_total Successful ECR registry logins.\n\
         # TYPE dd_build_server_ecr_logins_total counter\n\
         dd_build_server_ecr_logins_total {}\n\
         # HELP dd_build_server_ecr_login_failures_total Failed ECR registry logins.\n\
         # TYPE dd_build_server_ecr_login_failures_total counter\n\
         dd_build_server_ecr_login_failures_total {}\n",
        state.counters.submitted.load(Ordering::Relaxed),
        state.counters.running.load(Ordering::Relaxed),
        queued,
        state.counters.succeeded.load(Ordering::Relaxed),
        state.counters.failed.load(Ordering::Relaxed),
        state.counters.rejected.load(Ordering::Relaxed),
        state.counters.command_failures.load(Ordering::Relaxed),
        state.counters.ecr_logins.load(Ordering::Relaxed),
        state.counters.ecr_login_failures.load(Ordering::Relaxed),
    );
    body.push_str(&format!(
        "# HELP dd_build_server_dependencies_ready Whether auth, work storage, and required build tools are available.\n\
         # TYPE dd_build_server_dependencies_ready gauge\n\
         dd_build_server_dependencies_ready {}\n",
        u8::from(build_dependencies_ready(&state.config))
    ));
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

async fn submit_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BuildRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if let Err(error) = validate_build_request(&state.config, &request) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }

    let counter = state.counters.submitted.fetch_add(1, Ordering::Relaxed) + 1;
    let id = job_id(counter);
    let job_dir = state.config.work_root.join(&id);
    let log_path = job_dir.join("build.log");
    let record = BuildJobRecord {
        id: id.clone(),
        status: BuildStatus::Queued,
        request,
        created_at_ms: now_ms(),
        started_at_ms: None,
        finished_at_ms: None,
        log_path: log_path.to_string_lossy().to_string(),
        error: None,
    };

    {
        let mut jobs = state.jobs.write().await;
        jobs.insert(id.clone(), record.clone());
    }
    prune_jobs(&state).await;

    let task_state = state.clone();
    tokio::spawn(async move {
        run_job(task_state, id).await;
    });

    (StatusCode::ACCEPTED, Json(record)).into_response()
}

async fn list_builds(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let mut jobs = state
        .jobs
        .read()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    jobs.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Json(jobs).into_response()
}

async fn get_build(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let jobs = state.jobs.read().await;
    match jobs.get(&job_id) {
        Some(job) => Json(job).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "build job not found" })),
        )
            .into_response(),
    }
}

async fn get_build_logs(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let log_path = {
        let jobs = state.jobs.read().await;
        match jobs.get(&job_id) {
            Some(job) => PathBuf::from(&job.log_path),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "build job not found" })),
                )
                    .into_response();
            }
        }
    };

    match fs::read_to_string(&log_path).await {
        Ok(body) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "build log not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to read build log: {error}") })),
        )
            .into_response(),
    }
}

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init("dd-build-server");

    let config = Arc::new(config_from_env());
    let host = env_value("HOST", "0.0.0.0");
    let port = env_u64("PORT", DEFAULT_PORT as u64) as u16;
    let max_concurrent = env_usize("BUILD_SERVER_MAX_CONCURRENT_BUILDS", 1);

    if let Err(error) = fs::create_dir_all(&config.work_root).await {
        panic!("failed to create build server work root: {error}");
    }

    let state = AppState {
        config,
        http: reqwest::Client::new(),
        jobs: Arc::new(RwLock::new(HashMap::new())),
        semaphore: Arc::new(Semaphore::new(max_concurrent)),
        counters: Arc::new(Counters::default()),
    };

    let app = Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/builds", get(list_builds).post(submit_build))
        .route("/builds/:job_id", get(get_build))
        .route("/builds/:job_id/logs", get(get_build_logs))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    tracing::info!("{SERVICE_NAME} listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_and_path_validation_blocks_command_and_path_injection() {
        assert!(validate_repo_url("https://github.com/ORESoftware/example.git").is_ok());
        assert!(validate_repo_url("file:///etc/passwd").is_err());
        assert!(validate_repo_url("https://github.com/example.git\n--upload-pack=evil").is_err());
        assert!(validate_relative_path("contextDir", "services/api").is_ok());
        assert!(validate_relative_path("contextDir", "../../etc").is_err());
        assert!(validate_relative_path("contextDir", "/etc").is_err());
    }

    #[test]
    fn build_args_reject_secret_like_keys() {
        let safe = Some(BTreeMap::from([(
            "BUILD_PROFILE".to_string(),
            "release".to_string(),
        )]));
        let unsafe_args = Some(BTreeMap::from([(
            "GITHUB_TOKEN".to_string(),
            "do-not-pass-secrets-as-build-args".to_string(),
        )]));
        assert!(validate_build_args(&safe).is_ok());
        assert!(validate_build_args(&unsafe_args).is_err());
    }

    #[test]
    fn executable_lookup_accepts_path_commands_and_rejects_missing_tools() {
        assert!(executable_available("sh"));
        assert!(!executable_available(
            "dd-build-server-tool-that-does-not-exist"
        ));
    }
}
