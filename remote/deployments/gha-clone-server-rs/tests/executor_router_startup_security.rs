use std::{
    fs,
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread::sleep,
    time::Duration,
};

use serde_json::json;
use uuid::Uuid;

const ROUTER_BINARY: &str = env!("CARGO_BIN_EXE_gha-executor-router");
const VALID_ROUTER_AUTH: &str = "router-auth-secret-with-at-least-32-bytes";
const VALID_EXECUTOR_AUTH: &str = "executor-auth-secret-with-at-least-32-bytes";
const MULTILINE_ROUTER_AUTH: &str =
    "router-auth-secret-first-half\nrouter-auth-secret-second-half";
const MULTILINE_EXECUTOR_AUTH: &str =
    "executor-auth-secret-first-half\r\nexecutor-auth-secret-second-half";

fn unique_secret_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "gha-executor-router-startup-security-{}",
        Uuid::new_v4()
    ))
}

fn unused_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("reserve startup-security port")
        .local_addr()
        .expect("read startup-security address")
        .port()
}

fn write_secret(root: &Path, name: &str, value: &str) -> PathBuf {
    fs::create_dir_all(root).expect("create mounted-secret root");
    let path = root.join(name);
    fs::write(&path, value).expect("write mounted secret fixture");
    path
}

fn run_until_rejected(
    root: &Path,
    router_auth_path: &Path,
    executor_auth_path: &Path,
) -> (ExitStatus, String) {
    let specs = json!([{
        "id": "aws-primary",
        "provider": "aws",
        "enabled": true,
        "url": "http://127.0.0.1:1",
        "authPath": executor_auth_path
    }]);
    let mut child = Command::new(ROUTER_BINARY)
        .env_clear()
        .env("HOST", "127.0.0.1")
        .env("PORT", unused_port().to_string())
        .env("GHA_EXECUTOR_ROUTER_SECRET_ROOT", root)
        .env("GHA_EXECUTOR_ROUTER_AUTH_PATH", router_auth_path)
        .env("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON", specs.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start router with rejected secret fixture");

    for _ in 0..200 {
        if let Some(status) = child.try_wait().expect("poll rejected router startup") {
            let mut stderr = String::new();
            child
                .stderr
                .take()
                .expect("router stderr pipe")
                .read_to_string(&mut stderr)
                .expect("read rejected router stderr");
            return (status, stderr);
        }
        sleep(Duration::from_millis(10));
    }

    let _ = child.kill();
    let _ = child.wait();
    panic!("router did not reject the malformed mounted secret before binding");
}

fn assert_source_redacted_rejection(status: ExitStatus, stderr: &str) {
    assert_eq!(status.code(), Some(2));
    assert!(stderr.contains("configuration error"));
    for secret in [
        VALID_ROUTER_AUTH,
        VALID_EXECUTOR_AUTH,
        MULTILINE_ROUTER_AUTH,
        MULTILINE_EXECUTOR_AUTH,
    ] {
        assert!(!stderr.contains(secret), "startup stderr leaked a secret");
    }
}

#[test]
fn multiline_inbound_router_secret_exits_before_binding_without_leaking() {
    let root = unique_secret_root();
    let router_auth = write_secret(&root, "router-auth", MULTILINE_ROUTER_AUTH);
    let executor_auth = write_secret(&root, "aws-auth", VALID_EXECUTOR_AUTH);

    let (status, stderr) = run_until_rejected(&root, &router_auth, &executor_auth);
    assert_source_redacted_rejection(status, &stderr);

    fs::remove_dir_all(root).expect("remove inbound-secret fixture");
}

#[test]
fn multiline_executor_secret_exits_before_binding_without_leaking() {
    let root = unique_secret_root();
    let router_auth = write_secret(&root, "router-auth", VALID_ROUTER_AUTH);
    let executor_auth = write_secret(&root, "aws-auth", MULTILINE_EXECUTOR_AUTH);

    let (status, stderr) = run_until_rejected(&root, &router_auth, &executor_auth);
    assert_source_redacted_rejection(status, &stderr);

    fs::remove_dir_all(root).expect("remove executor-secret fixture");
}
