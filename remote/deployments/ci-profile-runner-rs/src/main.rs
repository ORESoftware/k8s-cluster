use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{
    fs,
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::Semaphore,
    time::timeout,
};

const SERVICE: &str = "dd-ci-profile-runner";
const SCHEMA: &str = "ci-profile-runner.v1";
const PLAYWRIGHT_IMAGE: &str = "mcr.microsoft.com/playwright:v1.60.0-noble";
const RUST_IMAGE: &str = "docker.io/library/rust:1.90-bookworm";
const PLAYWRIGHT_SCRIPT: &str = "npm ci && npx playwright test";
const PUPPETEER_SCRIPT: &str = "npm ci && npm run test:puppeteer";
const RUST_VERIFY_SCRIPT: &str = r#"set -euo pipefail
crate_dir=.
if [ ! -f "$crate_dir/Cargo.toml" ]; then
  if [ -f remote/deployments/gha-clone-server-rs/Cargo.toml ]; then
    crate_dir=remote/deployments/gha-clone-server-rs
  elif [ -f generated/rust/Cargo.toml ] \
    && [ -f generated/rust/Cargo.lock ] \
    && [ -f schema/domain.schema.json ] \
    && [ -f nats/subjects.json ]; then
    crate_dir=generated/rust
  else
    echo "rust-verify requires Cargo.toml at repository root, the reviewed gha-clone-server monorepo path, or the reviewed generated-interface crate shape" >&2
    exit 2
  fi
fi
cd "$crate_dir"
rustup component add rustfmt clippy
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features"#;

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    server_auth_secret: String,
    work_root: PathBuf,
    git_bin: String,
    nerdctl_bin: String,
    containerd_namespace: String,
    network: String,
    allowed: HashMap<String, String>,
    max_concurrent: usize,
    max_seconds: u64,
    max_output_bytes: usize,
    cpus: String,
    memory: String,
    pids_limit: String,
    shm_size: String,
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    semaphore: Arc<Semaphore>,
    counter: Arc<AtomicU64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunRequest {
    schema_version: String,
    request_id: Option<String>,
    repository: String,
    revision: String,
    profile: String,
}

#[derive(Debug)]
struct CommandResult {
    success: bool,
    exit_code: Option<i32>,
    output: String,
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_value(name: &str, fallback: &str) -> String {
    env_string(name).unwrap_or_else(|| fallback.to_string())
}

fn env_u64(name: &str, fallback: u64) -> u64 {
    env_string(name)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_usize(name: &str, fallback: usize) -> usize {
    env_string(name)
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
}

fn constant_time_equals(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

fn authorized(headers: &HeaderMap, config: &Config) -> bool {
    ["x-server-auth", "authorization", "x-auth"]
        .iter()
        .filter_map(|name| headers.get(*name))
        .filter_map(|value| value.to_str().ok())
        .map(|value| {
            value
                .trim_start_matches("Bearer ")
                .trim_start_matches("bearer ")
        })
        .any(|candidate| constant_time_equals(candidate, &config.server_auth_secret))
}

fn valid_repository(value: &str) -> bool {
    let mut parts = value.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(repo) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || owner.is_empty() || repo.is_empty() {
        return false;
    }
    [owner, repo].iter().all(|part| {
        !part.starts_with('.')
            && !part.ends_with('.')
            && !part.contains("..")
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    })
}

fn valid_revision(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn profile_definition(profile: &str) -> Option<(&'static str, &'static str)> {
    match profile {
        "playwright" => Some((PLAYWRIGHT_IMAGE, PLAYWRIGHT_SCRIPT)),
        "puppeteer" => Some((PLAYWRIGHT_IMAGE, PUPPETEER_SCRIPT)),
        "rust-verify" => Some((RUST_IMAGE, RUST_VERIFY_SCRIPT)),
        _ => None,
    }
}

fn parse_allowed_rules(raw: &str) -> HashMap<String, String> {
    serde_json::from_str::<HashMap<String, String>>(raw)
        .unwrap_or_else(|error| panic!("invalid CI_PROFILE_RUNNER_RULES_JSON: {error}"))
        .into_iter()
        .map(|(repository, profile)| {
            if !valid_repository(&repository) {
                panic!("invalid repository in CI_PROFILE_RUNNER_RULES_JSON: {repository:?}");
            }
            if profile_definition(&profile).is_none() {
                panic!("invalid profile in CI_PROFILE_RUNNER_RULES_JSON: {profile:?}");
            }
            (repository, profile)
        })
        .collect()
}

fn config_from_env() -> Config {
    let server_auth_secret = env_string("SERVER_AUTH_SECRET")
        .or_else(|| env_string("CI_PROFILE_RUNNER_AUTH_SECRET"))
        .unwrap_or_else(|| panic!("SERVER_AUTH_SECRET is required"));
    let allowed = parse_allowed_rules(
        &env_string("CI_PROFILE_RUNNER_RULES_JSON").unwrap_or_else(|| "{}".to_string()),
    );
    if allowed.is_empty() {
        panic!(
            "CI_PROFILE_RUNNER_RULES_JSON must contain at least one exact repository/profile rule"
        );
    }
    Config {
        host: env_value("HOST", "0.0.0.0"),
        port: env_value("PORT", "8147").parse::<u16>().unwrap_or(8147),
        server_auth_secret,
        work_root: PathBuf::from(env_value(
            "CI_PROFILE_RUNNER_WORK_ROOT",
            "/var/lib/dd-ci-profile-runner/jobs",
        )),
        git_bin: env_value("CI_PROFILE_RUNNER_GIT_BIN", "git"),
        nerdctl_bin: env_value("CI_PROFILE_RUNNER_NERDCTL_BIN", "/usr/local/bin/nerdctl"),
        containerd_namespace: env_value("CI_PROFILE_RUNNER_CONTAINERD_NAMESPACE", "dd-ci-profiles"),
        network: env_value("CI_PROFILE_RUNNER_NETWORK", "host"),
        allowed,
        max_concurrent: env_usize("CI_PROFILE_RUNNER_MAX_CONCURRENT", 2).clamp(1, 4),
        max_seconds: env_u64("CI_PROFILE_RUNNER_MAX_SECONDS", 1_200).clamp(60, 1_800),
        max_output_bytes: env_usize("CI_PROFILE_RUNNER_MAX_OUTPUT_BYTES", 64 * 1024)
            .clamp(4 * 1024, 256 * 1024),
        cpus: env_value("CI_PROFILE_RUNNER_CPUS", "2"),
        memory: env_value("CI_PROFILE_RUNNER_MEMORY", "4g"),
        pids_limit: env_value("CI_PROFILE_RUNNER_PIDS_LIMIT", "2048"),
        shm_size: env_value("CI_PROFILE_RUNNER_SHM_SIZE", "1g"),
    }
}

async fn read_tail<R>(mut reader: R, max_bytes: usize) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut kept = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if read >= max_bytes {
                    kept.clear();
                    kept.extend_from_slice(&chunk[read - max_bytes..read]);
                    continue;
                }
                if kept.len() + read > max_bytes {
                    let remove = kept.len() + read - max_bytes;
                    kept.drain(..remove);
                }
                kept.extend_from_slice(&chunk[..read]);
            }
        }
    }
    kept
}

async fn run_capped(
    program: &str,
    args: Vec<String>,
    cwd: &Path,
    max_seconds: u64,
    max_output_bytes: usize,
) -> Result<CommandResult, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.env_remove("SERVER_AUTH_SECRET");
    command.env_remove("CI_PROFILE_RUNNER_AUTH_SECRET");

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start {program}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture stderr".to_string())?;
    let stdout_task = tokio::spawn(read_tail(stdout, max_output_bytes / 2));
    let stderr_task = tokio::spawn(read_tail(stderr, max_output_bytes / 2));

    let status = match timeout(Duration::from_secs(max_seconds), child.wait()).await {
        Ok(result) => result.map_err(|error| format!("failed waiting for {program}: {error}"))?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(format!("{program} timed out after {max_seconds}s"));
        }
    };
    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();
    let mut output = String::from_utf8_lossy(&stdout).to_string();
    if !stderr.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&String::from_utf8_lossy(&stderr));
    }
    Ok(CommandResult {
        success: status.success(),
        exit_code: status.code(),
        output,
    })
}

fn restricted_git_args() -> Vec<String> {
    vec![
        "-c".to_string(),
        "protocol.ext.allow=never".to_string(),
        "-c".to_string(),
        "protocol.file.allow=never".to_string(),
        "-c".to_string(),
        "protocol.local.allow=never".to_string(),
    ]
}

async fn git_checked(config: &Config, args: Vec<String>, cwd: &Path) -> Result<(), String> {
    let result = run_capped(&config.git_bin, args, cwd, 120, config.max_output_bytes).await?;
    if result.success {
        Ok(())
    } else {
        Err(format!(
            "git exited {:?}: {}",
            result.exit_code,
            result.output.trim()
        ))
    }
}

async fn clone_exact(
    config: &Config,
    repository: &str,
    revision: &str,
    job_dir: &Path,
    repo_dir: &Path,
) -> Result<(), String> {
    fs::create_dir_all(job_dir)
        .await
        .map_err(|error| format!("failed to create job directory: {error}"))?;
    let mut init = restricted_git_args();
    init.extend([
        "init".to_string(),
        "--".to_string(),
        repo_dir.to_string_lossy().to_string(),
    ]);
    git_checked(config, init, job_dir).await?;
    git_checked(
        config,
        vec![
            "-C".to_string(),
            repo_dir.to_string_lossy().to_string(),
            "remote".to_string(),
            "add".to_string(),
            "origin".to_string(),
            format!("https://github.com/{repository}.git"),
        ],
        job_dir,
    )
    .await?;
    let mut fetch = restricted_git_args();
    fetch.extend([
        "-C".to_string(),
        repo_dir.to_string_lossy().to_string(),
        "fetch".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--no-tags".to_string(),
        "--no-recurse-submodules".to_string(),
        "origin".to_string(),
        revision.to_string(),
    ]);
    git_checked(config, fetch, job_dir).await?;
    git_checked(
        config,
        vec![
            "-C".to_string(),
            repo_dir.to_string_lossy().to_string(),
            "checkout".to_string(),
            "--detach".to_string(),
            "--force".to_string(),
            "FETCH_HEAD".to_string(),
        ],
        job_dir,
    )
    .await?;
    let result = run_capped(
        &config.git_bin,
        vec![
            "-C".to_string(),
            repo_dir.to_string_lossy().to_string(),
            "rev-parse".to_string(),
            "HEAD".to_string(),
        ],
        job_dir,
        30,
        4096,
    )
    .await?;
    if !result.success || result.output.trim() != revision {
        return Err("checked out revision does not match requested immutable commit".to_string());
    }
    Ok(())
}

async fn force_remove(config: &Config, container_name: &str, cwd: &Path) {
    let _ = run_capped(
        &config.nerdctl_bin,
        vec![
            "-n".to_string(),
            config.containerd_namespace.clone(),
            "rm".to_string(),
            "-f".to_string(),
            container_name.to_string(),
        ],
        cwd,
        30,
        4096,
    )
    .await;
}

async fn run_profile(
    config: &Config,
    repository: &str,
    revision: &str,
    profile: &str,
    container_name: &str,
    repo_dir: &Path,
) -> Result<CommandResult, String> {
    let (image, script) = profile_definition(profile)
        .ok_or_else(|| format!("profile {profile:?} is not installed"))?;
    let deadline_ms = now_ms() + u128::from(config.max_seconds) * 1000;
    let args = vec![
        "-n".to_string(),
        config.containerd_namespace.clone(),
        "run".to_string(),
        "--rm".to_string(),
        "--pull=missing".to_string(),
        "--name".to_string(),
        container_name.to_string(),
        "--label".to_string(),
        "dd.ci-profile.managed=true".to_string(),
        "--label".to_string(),
        format!("dd.ci-profile.repository={repository}"),
        "--label".to_string(),
        format!("dd.ci-profile.revision={revision}"),
        "--label".to_string(),
        format!("dd.ci-profile.profile={profile}"),
        "--label".to_string(),
        format!("dd.ci-profile.deadline-ms={deadline_ms}"),
        "--network".to_string(),
        config.network.clone(),
        format!("--cpus={}", config.cpus),
        format!("--memory={}", config.memory),
        format!("--pids-limit={}", config.pids_limit),
        format!("--shm-size={}", config.shm_size),
        "--security-opt=no-new-privileges".to_string(),
        "--cap-drop=ALL".to_string(),
        "--env=CI=true".to_string(),
        "--mount".to_string(),
        format!(
            "type=bind,src={},dst=/workspace",
            repo_dir.to_string_lossy()
        ),
        "--workdir".to_string(),
        "/workspace".to_string(),
        image.to_string(),
        "/bin/bash".to_string(),
        "-lc".to_string(),
        script.to_string(),
    ];
    match run_capped(
        &config.nerdctl_bin,
        args,
        repo_dir,
        config.max_seconds,
        config.max_output_bytes,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(error) => {
            force_remove(config, container_name, repo_dir).await;
            Err(error)
        }
    }
}

fn validate_request(request: &RunRequest, config: &Config) -> Result<(), String> {
    if request.schema_version != SCHEMA {
        return Err(format!("schemaVersion must be {SCHEMA}"));
    }
    if !valid_repository(&request.repository) {
        return Err(
            "repository must be owner/name with a conservative GitHub-safe spelling".to_string(),
        );
    }
    if !valid_revision(&request.revision) {
        return Err("revision must be an exact 40-hex commit SHA".to_string());
    }
    let expected = config
        .allowed
        .get(&request.repository)
        .ok_or_else(|| "repository is not allowlisted for CI profile execution".to_string())?;
    if expected != &request.profile {
        return Err(format!(
            "repository is bound to profile {expected:?}, not {:?}",
            request.profile
        ));
    }
    if profile_definition(&request.profile).is_none() {
        return Err("profile is not installed".to_string());
    }
    if let Some(request_id) = request.request_id.as_deref() {
        if request_id.len() > 160
            || !request_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err("requestId contains unsupported characters or is too long".to_string());
        }
    }
    Ok(())
}

async fn handle_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RunRequest>,
) -> impl IntoResponse {
    if !authorized(&headers, &state.config) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"ok": false, "error": "unauthorized"})),
        );
    }
    if let Err(error) = validate_request(&request, &state.config) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": error})),
        );
    }
    let permit = match state.semaphore.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"ok": false, "error": "profile runner concurrency limit reached"})),
            );
        }
    };

    let sequence = state.counter.fetch_add(1, Ordering::Relaxed);
    let runner_id = format!("{:x}-{:04x}", now_ms(), sequence & 0xffff);
    let job_dir = state.config.work_root.join(&runner_id);
    let repo_dir = job_dir.join("repo");
    let container_name = format!("dd-ci-profile-{runner_id}");
    let started_ms = now_ms();

    let result = async {
        clone_exact(
            &state.config,
            &request.repository,
            &request.revision,
            &job_dir,
            &repo_dir,
        )
        .await?;
        run_profile(
            &state.config,
            &request.repository,
            &request.revision,
            &request.profile,
            &container_name,
            &repo_dir,
        )
        .await
    }
    .await;

    force_remove(&state.config, &container_name, &job_dir).await;
    let _ = fs::remove_dir_all(&job_dir).await;
    drop(permit);

    match result {
        Ok(command) if command.success => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "schemaVersion": SCHEMA,
                "requestId": request.request_id,
                "repository": request.repository,
                "revision": request.revision,
                "profile": request.profile,
                "image": profile_definition(&request.profile).map(|value| value.0),
                "runnerId": runner_id,
                "durationMs": now_ms().saturating_sub(started_ms),
                "exitCode": command.exit_code,
                "outputTail": command.output,
            })),
        ),
        Ok(command) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "ok": false,
                "schemaVersion": SCHEMA,
                "requestId": request.request_id,
                "repository": request.repository,
                "revision": request.revision,
                "profile": request.profile,
                "runnerId": runner_id,
                "durationMs": now_ms().saturating_sub(started_ms),
                "exitCode": command.exit_code,
                "outputTail": command.output,
                "error": "fixed profile command failed",
            })),
        ),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "ok": false,
                "schemaVersion": SCHEMA,
                "requestId": request.request_id,
                "repository": request.repository,
                "revision": request.revision,
                "profile": request.profile,
                "runnerId": runner_id,
                "durationMs": now_ms().saturating_sub(started_ms),
                "error": error,
            })),
        ),
    }
}

fn descriptor(state: &AppState) -> Value {
    json!({
        "service": SERVICE,
        "ok": true,
        "schemaVersion": SCHEMA,
        "profiles": ["playwright", "puppeteer", "rust-verify"],
        "repositories": state.config.allowed.keys().collect::<Vec<_>>(),
        "maxConcurrent": state.config.max_concurrent,
        "maxSeconds": state.config.max_seconds,
    })
}

fn router(state: AppState) -> Router {
    let descriptor_state = state.clone();
    Router::new()
        .route(
            "/",
            get(move || {
                let state = descriptor_state.clone();
                async move { Json(descriptor(&state)) }
            }),
        )
        .route(
            "/healthz",
            get(|| async { Json(json!({"ok": true, "service": SERVICE})) }),
        )
        .route(
            "/readyz",
            get(|| async { Json(json!({"ok": true, "service": SERVICE})) }),
        )
        .route("/run", post(handle_run))
        .with_state(state)
}

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init(SERVICE);
    let config = Arc::new(config_from_env());
    fs::create_dir_all(&config.work_root)
        .await
        .unwrap_or_else(|error| panic!("failed to create work root: {error}"));
    let state = AppState {
        semaphore: Arc::new(Semaphore::new(config.max_concurrent)),
        counter: Arc::new(AtomicU64::new(0)),
        config: config.clone(),
    };
    let bind = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {bind}: {error}"));
    tracing::info!("{SERVICE} listening on {bind}");
    axum::serve(
        listener,
        router(state).layer(dd_telemetry::http_trace_layer()),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .expect("server error");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_revision_required() {
        assert!(valid_revision("1e1116ef6811c4e3e6be34ad3e1def39bc20ef59"));
        assert!(!valid_revision("main"));
        assert!(!valid_revision("1e1116"));
    }

    #[test]
    fn repository_spelling_is_conservative() {
        assert!(valid_repository(
            "discrete-event-systems-test/des-web-playwright-e2e"
        ));
        assert!(!valid_repository("discrete-event-systems-test/../secret"));
        assert!(!valid_repository("https://github.com/a/b"));
    }

    #[test]
    fn profile_scripts_are_fixed() {
        assert_eq!(
            profile_definition("playwright"),
            Some((PLAYWRIGHT_IMAGE, PLAYWRIGHT_SCRIPT))
        );
        assert_eq!(
            profile_definition("puppeteer"),
            Some((PLAYWRIGHT_IMAGE, PUPPETEER_SCRIPT))
        );
        assert_eq!(
            profile_definition("rust-verify"),
            Some((RUST_IMAGE, RUST_VERIFY_SCRIPT))
        );
        assert!(profile_definition("arbitrary").is_none());
    }

    #[test]
    fn exact_repo_profile_binding_is_enforced() {
        let mut allowed = HashMap::new();
        allowed.insert("o/p".to_string(), "playwright".to_string());
        let config = Config {
            host: "127.0.0.1".to_string(),
            port: 8147,
            server_auth_secret: "secret".to_string(),
            work_root: PathBuf::from("/tmp"),
            git_bin: "git".to_string(),
            nerdctl_bin: "nerdctl".to_string(),
            containerd_namespace: "test".to_string(),
            network: "none".to_string(),
            allowed,
            max_concurrent: 1,
            max_seconds: 60,
            max_output_bytes: 4096,
            cpus: "1".to_string(),
            memory: "1g".to_string(),
            pids_limit: "128".to_string(),
            shm_size: "256m".to_string(),
        };
        let request = RunRequest {
            schema_version: SCHEMA.to_string(),
            request_id: None,
            repository: "o/p".to_string(),
            revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
            profile: "puppeteer".to_string(),
        };
        assert!(validate_request(&request, &config).is_err());
    }
}
