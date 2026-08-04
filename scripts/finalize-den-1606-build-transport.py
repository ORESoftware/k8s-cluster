#!/usr/bin/env python3
"""Apply the reviewed DEN-1606 transport boundary to exact current dev."""

from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label} drifted: expected one match, found {count}")
    return text.replace(old, new, 1)


main_path = Path("remote/deployments/gha-clone-server-rs/src/main.rs")
main = main_path.read_text(encoding="utf-8")

main = replace_once(
    main,
    '''            build_server_url: env_optional("GHA_CLONE_BUILD_SERVER_URL")
                .map(|value| value.trim_end_matches('/').to_string()),
''',
    '''            build_server_url: build_server_url_from_env()?,
''',
    "build-server URL configuration",
)
main = replace_once(
    main,
    '''                max_workflow_bytes: env_usize(
''',
    '''                max_workflow_bytes: env_nonzero_usize(
''',
    "workflow-byte bound",
)
main = replace_once(
    main,
    '''                max_jobs: env_usize("GHA_CLONE_MAX_JOBS", gha_clone_server::MAX_JOBS_DEFAULT)?,
''',
    '''                max_jobs: env_nonzero_usize(
                    "GHA_CLONE_MAX_JOBS",
                    gha_clone_server::MAX_JOBS_DEFAULT,
                )?,
''',
    "job-count bound",
)
main = replace_once(
    main,
    '''                max_steps_per_job: env_usize(
''',
    '''                max_steps_per_job: env_nonzero_usize(
''',
    "step-count bound",
)
main = replace_once(
    main,
    '''            build_poll_seconds: env_u64("GHA_CLONE_BUILD_POLL_SECONDS", 2)?,
''',
    '''            build_poll_seconds: env_nonzero_u64("GHA_CLONE_BUILD_POLL_SECONDS", 2)?,
''',
    "poll interval",
)
main = replace_once(
    main,
    '''            build_timeout_seconds: env_u64("GHA_CLONE_BUILD_TIMEOUT_SECONDS", 3600)?,
''',
    '''            build_timeout_seconds: env_nonzero_u64(
                "GHA_CLONE_BUILD_TIMEOUT_SECONDS",
                3600,
            )?,
''',
    "build timeout",
)

response_struct = '''struct BuildJobResponse {
    id: String,
    status: String,
    error: Option<String>,
}
'''
response_validation = response_struct + '''
fn valid_build_job_id(value: &str) -> bool {
    !matches!(value, "" | "." | "..")
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn validate_build_job_response_id(
    build: &BuildJobResponse,
    expected: Option<&str>,
) -> Result<(), String> {
    if !valid_build_job_id(&build.id) {
        return Err("build server returned an invalid job ID".into());
    }
    if expected.is_some_and(|expected| build.id != expected) {
        return Err("build status returned a mismatched job ID".into());
    }
    Ok(())
}
'''
main = replace_once(
    main,
    response_struct,
    response_validation,
    "build response identity helper insertion",
)

main = replace_once(
    main,
    '''        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .user_agent("gha-clone-server/0.1")
            .build()
            .expect("reqwest client"),
''',
    '''        client: build_http_client().expect("reqwest client"),
''',
    "HTTP client construction",
)
main = replace_once(
    main,
    '''fn router(state: AppState) -> Router {
''',
    '''fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("gha-clone-server/0.1")
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))
}

fn router(state: AppState) -> Router {
''',
    "no-redirect client helper",
)

submit_parse = '''        let build: BuildJobResponse = serde_json::from_str(&body)
            .map_err(|error| format!("build server returned invalid job JSON: {error}"))?;
'''
main = replace_once(
    main,
    submit_parse,
    submit_parse + '''        validate_build_job_response_id(&build, None)?;
''',
    "accepted build ID validation",
)
status_parse = '''        let build: BuildJobResponse = serde_json::from_str(&body)
            .map_err(|error| format!("build status JSON is invalid: {error}"))?;
'''
main = replace_once(
    main,
    status_parse,
    status_parse + '''        validate_build_job_response_id(&build, Some(build_job_id))?;
''',
    "polled build ID validation",
)

helper_marker = '''fn github_api_base_url_from_env() -> Result<String, String> {
'''
helper = '''fn build_server_url_from_env() -> Result<Option<String>, String> {
    env_optional("GHA_CLONE_BUILD_SERVER_URL")
        .map(|value| normalize_build_server_url(&value))
        .transpose()
}

fn valid_dns_label(value: &str) -> bool {
    let bytes = value.as_bytes();
    let (Some(first), Some(last)) = (bytes.first(), bytes.last()) else {
        return false;
    };
    bytes.len() <= 63
        && first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn is_kubernetes_service_dns(host: &str) -> bool {
    let labels = host.split('.').collect::<Vec<_>>();
    match labels.as_slice() {
        [service, namespace, "svc"] | [service, namespace, "svc", "cluster", "local"] => {
            valid_dns_label(service) && valid_dns_label(namespace)
        }
        _ => false,
    }
}

fn normalize_build_server_url(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return Err("GHA_CLONE_BUILD_SERVER_URL must not be empty".into());
    }
    let parsed = reqwest::Url::parse(value)
        .map_err(|error| format!("GHA_CLONE_BUILD_SERVER_URL is invalid: {error}"))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "GHA_CLONE_BUILD_SERVER_URL must not contain credentials, query, or fragment".into(),
        );
    }
    if parsed.path() != "/" {
        return Err("GHA_CLONE_BUILD_SERVER_URL must be an origin without a path".into());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "GHA_CLONE_BUILD_SERVER_URL must contain a host".to_string())?;
    let loopback_http = parsed.scheme() == "http"
        && matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]");
    let kubernetes_http = parsed.scheme() == "http" && is_kubernetes_service_dns(host);
    if parsed.scheme() != "https" && !loopback_http && !kubernetes_http {
        return Err(
            "GHA_CLONE_BUILD_SERVER_URL must use HTTPS; HTTP is allowed only for loopback tests or Kubernetes service DNS"
                .into(),
        );
    }
    Ok(value.to_string())
}

'''
main = replace_once(
    main,
    helper_marker,
    helper + helper_marker,
    "build-server origin validation helpers",
)

tests = '''

    #[test]
    fn build_server_origin_is_credential_free_and_transport_safe() {
        assert_eq!(
            normalize_build_server_url("https://build.example.com/").unwrap(),
            "https://build.example.com"
        );
        assert!(normalize_build_server_url("http://127.0.0.1:8123").is_ok());
        assert!(normalize_build_server_url("http://localhost:8123").is_ok());
        assert!(
            normalize_build_server_url("http://dd-build-server.remote.svc.cluster.local:8123")
                .is_ok()
        );
        assert!(normalize_build_server_url("http://dd-build-server.remote.svc:8123").is_ok());
        assert!(normalize_build_server_url("http://[::1]:8123").is_ok());
        assert!(
            normalize_build_server_url("http://extra.dd-build-server.remote.svc:8123")
                .is_err()
        );
        assert!(normalize_build_server_url("http://dd_build.remote.svc:8123").is_err());
        assert!(normalize_build_server_url("http://-build.remote.svc:8123").is_err());
        assert!(normalize_build_server_url("http://10.0.0.10:8123").is_err());
        assert!(normalize_build_server_url("http://build.example.com").is_err());
        assert!(normalize_build_server_url("http://service.svc.evil.example").is_err());
        assert!(normalize_build_server_url("https://user:pass@build.example.com").is_err());
        assert!(normalize_build_server_url("https://build.example.com/api").is_err());
        assert!(normalize_build_server_url("https://build.example.com?token=x").is_err());
    }

    #[test]
    fn build_job_response_identity_is_validated_and_bound() {
        let valid = BuildJobResponse {
            id: "build:0123-abc_def.test".into(),
            status: "queued".into(),
            error: None,
        };
        assert!(validate_build_job_response_id(&valid, None).is_ok());
        assert!(validate_build_job_response_id(&valid, Some(&valid.id)).is_ok());
        assert!(validate_build_job_response_id(&valid, Some("different")).is_err());

        for id in ["", ".", "..", "../build?token=x", "build/child"] {
            let invalid = BuildJobResponse {
                id: id.into(),
                status: "queued".into(),
                error: None,
            };
            assert!(validate_build_job_response_id(&invalid, None).is_err());
        }
        let too_long = BuildJobResponse {
            id: "a".repeat(129),
            status: "queued".into(),
            error: None,
        };
        assert!(validate_build_job_response_id(&too_long, None).is_err());
    }

    #[tokio::test]
    async fn configured_http_client_does_not_follow_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/start",
                get(|| async { axum::response::Redirect::temporary("/final") }),
            )
            .route("/final", get(|| async { StatusCode::OK }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let response = build_http_client()
            .unwrap()
            .get(format!("http://{address}/start"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        server.abort();
    }
'''
if not main.endswith("\n}\n"):
    raise SystemExit("test module closing boundary drifted")
main = main[:-3] + tests + "\n}\n"
main_path.write_text(main, encoding="utf-8")

replacements = {
    Path("remote/deployments/gha-clone-server-rs/tests/http_api.rs"): [
        (
            'env.insert("GHA_CLONE_BUILD_POLL_SECONDS", "0".to_string());',
            'env.insert("GHA_CLONE_BUILD_POLL_SECONDS", "1".to_string());',
        ),
        (
            '(MockMode::KeepRunning, 0, "exceeded 0 seconds"),',
            '(MockMode::KeepRunning, 1, "exceeded 1 seconds"),',
        ),
    ],
    Path("remote/deployments/gha-clone-server-rs/tests/streempilot_http.rs"): [
        (
            '.env("GHA_CLONE_BUILD_POLL_SECONDS", "0")',
            '.env("GHA_CLONE_BUILD_POLL_SECONDS", "1")',
        ),
    ],
    Path("remote/deployments/gha-clone-server-rs/tests/webhook_e2e.rs"): [
        (
            'env.insert("GHA_CLONE_BUILD_POLL_SECONDS", "0".to_string());',
            'env.insert("GHA_CLONE_BUILD_POLL_SECONDS", "1".to_string());',
        ),
    ],
    Path("remote/deployments/gha-clone-server-rs/tests/webhook_retention.rs"): [
        (
            '.env("GHA_CLONE_BUILD_POLL_SECONDS", "0")',
            '.env("GHA_CLONE_BUILD_POLL_SECONDS", "1")',
        ),
    ],
}
for path, pairs in replacements.items():
    text = path.read_text(encoding="utf-8")
    for old, new in pairs:
        text = replace_once(text, old, new, f"{path}:{old}")
    path.write_text(text, encoding="utf-8")

Path("docs/gha-clone-build-server-boundary.md").write_text(
    """# GHA clone build-server trust boundary

The independent clone-server submits only fixed, operator-reviewed profiles to `dd-build-server`. It is not a general GitHub Actions runner.

## Transport origin

`GHA_CLONE_BUILD_SERVER_URL` is an origin, not an arbitrary URL. It must contain no credentials, query, fragment, or non-root path. HTTPS is required except for loopback test servers and Kubernetes service DNS such as `service.namespace.svc` or `service.namespace.svc.cluster.local`. The HTTP exception accepts exactly those three- or five-label service forms, with DNS-safe service and namespace labels; extra-prefix lookalikes are rejected. HTTP redirects are disabled so authentication headers and immutable job identity cannot be moved to another origin.

## Build identity

Every accepted build response must contain a bounded path-safe identifier. Empty IDs and URL dot-segments (`.` and `..`) are rejected explicitly before a polling URL is formed. Poll responses must return the same identifier that was accepted at submission. Unknown, malformed, or mismatched identifiers fail the workflow run before URL construction or state mutation.

## Runtime bounds

Planner limits, polling interval, execution timeout, and retained-run capacity are strictly positive configuration values. Zero is configuration failure rather than an instruction to disable a safety bound.

This boundary complements the AWS/Hetzner executor router: provider selection happens before submission, and status remains pinned to the accepted provider and build identity.
""",
    encoding="utf-8",
)
