from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ROUTER = ROOT / "remote/deployments/gha-clone-server-rs/src/bin/executor_router.rs"
DEPLOYMENT = ROOT / "remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml"
CONTRACT = ROOT / "remote/tests/general/gha-clone-server-config.test.ts"
DOC = ROOT / "docs/gha-executor-router.md"


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


router = ROUTER.read_text()
router = replace_once(
    router,
    "const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024;\nconst MAX_ROUTES: usize = 8;",
    "const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024;\n"
    "const DEFAULT_MAX_UPSTREAM_RESPONSE_BYTES: usize = 1024 * 1024;\n"
    "const MAX_ROUTES: usize = 8;",
    "response-size constant",
)
router = replace_once(
    router,
    "    routes: Vec<ExecutorRoute>,\n    max_body_bytes: usize,\n}",
    "    routes: Vec<ExecutorRoute>,\n"
    "    max_body_bytes: usize,\n"
    "    max_upstream_response_bytes: usize,\n"
    "    failover_enabled: bool,\n"
    "    durable_idempotency_certified: bool,\n"
    "}",
    "config fields",
)
router = replace_once(
    router,
    '''        let max_body_bytes = env_usize(
            "GHA_EXECUTOR_ROUTER_MAX_BODY_BYTES",
            DEFAULT_MAX_BODY_BYTES,
        )?;
        if max_body_bytes == 0 || max_body_bytes > 4 * 1024 * 1024 {
            return Err(
                "GHA_EXECUTOR_ROUTER_MAX_BODY_BYTES must be between 1 and 4194304".to_string(),
            );
        }
        Ok(Self {
            auth_secret,
            routing_secret,
            routes,
            max_body_bytes,
        })''',
    '''        let max_body_bytes = env_usize(
            "GHA_EXECUTOR_ROUTER_MAX_BODY_BYTES",
            DEFAULT_MAX_BODY_BYTES,
        )?;
        if max_body_bytes == 0 || max_body_bytes > 4 * 1024 * 1024 {
            return Err(
                "GHA_EXECUTOR_ROUTER_MAX_BODY_BYTES must be between 1 and 4194304".to_string(),
            );
        }
        let max_upstream_response_bytes = env_usize(
            "GHA_EXECUTOR_ROUTER_MAX_UPSTREAM_RESPONSE_BYTES",
            DEFAULT_MAX_UPSTREAM_RESPONSE_BYTES,
        )?;
        if max_upstream_response_bytes == 0 || max_upstream_response_bytes > 8 * 1024 * 1024 {
            return Err(
                "GHA_EXECUTOR_ROUTER_MAX_UPSTREAM_RESPONSE_BYTES must be between 1 and 8388608"
                    .to_string(),
            );
        }
        let failover_enabled = env_bool("GHA_EXECUTOR_ROUTER_FAILOVER_ENABLED", false)?;
        let durable_idempotency_certified = env_bool(
            "GHA_EXECUTOR_ROUTER_DURABLE_IDEMPOTENCY_CERTIFIED",
            false,
        )?;
        validate_failover_policy(failover_enabled, durable_idempotency_certified)?;
        Ok(Self {
            auth_secret,
            routing_secret,
            routes,
            max_body_bytes,
            max_upstream_response_bytes,
            failover_enabled,
            durable_idempotency_certified,
        })''',
    "environment hardening",
)
router = replace_once(
    router,
    '''fn default_enabled() -> bool {
    true
}
''',
    '''fn default_enabled() -> bool {
    true
}

fn validate_failover_policy(
    failover_enabled: bool,
    durable_idempotency_certified: bool,
) -> Result<(), String> {
    if failover_enabled && !durable_idempotency_certified {
        return Err(
            "cross-executor failover requires GHA_EXECUTOR_ROUTER_DURABLE_IDEMPOTENCY_CERTIFIED=true"
                .to_string(),
        );
    }
    Ok(())
}
''',
    "failover policy",
)
router = replace_once(
    router,
    '''        "ok": true,
        "service": SERVICE_NAME,
        "routes": state.config.routes.len()
''',
    '''        "ok": true,
        "service": SERVICE_NAME,
        "routes": state.config.routes.len(),
        "failoverEnabled": state.config.failover_enabled,
        "durableIdempotencyCertified": state.config.durable_idempotency_certified
''',
    "health failover state",
)
router = replace_once(
    router,
    '''            "ok": ready,
            "service": SERVICE_NAME,
            "configuredRoutes": state.config.routes.len()
''',
    '''            "ok": ready,
            "service": SERVICE_NAME,
            "configuredRoutes": state.config.routes.len(),
            "failoverEnabled": state.config.failover_enabled,
            "durableIdempotencyCertified": state.config.durable_idempotency_certified
''',
    "readiness failover state",
)
router = replace_once(
    router,
    '''    let candidates = state
        .config
        .routes
        .iter()
        .filter(|route| route.supports(&metadata.profile))
        .collect::<Vec<_>>();
''',
    '''    let mut candidates = state
        .config
        .routes
        .iter()
        .filter(|route| route.supports(&metadata.profile))
        .collect::<Vec<_>>();
    if !state.config.failover_enabled {
        candidates.truncate(1);
    }
''',
    "single-authority routing",
)
router = replace_once(
    router,
    '''        let status = response.status();
        let response_body = match response.bytes().await {
            Ok(body) => body,
            Err(error) if retryable_status(status) => {
                attempts.push(json!({
                    "route": route.name,
                    "provider": route.provider,
                    "status": status.as_u16(),
                    "result": "retryable-response-body-error",
                    "detail": bounded_text(&error.to_string(), 256)
                }));
                continue;
            }
            Err(error) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "executor response body failed after a non-retryable response; request was not retried",
                        "route": route.name,
                        "provider": route.provider,
                        "status": status.as_u16(),
                        "detail": bounded_text(&error.to_string(), 512)
                    })),
                )
                    .into_response()
            }
        };
''',
    '''        let status = response.status();
        let response_body = match read_bounded_response_body(
            response,
            state.config.max_upstream_response_bytes,
        )
        .await
        {
            Ok(body) => body,
            Err(error) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "executor response body was invalid or exceeded the configured limit; request was not retried",
                        "route": route.name,
                        "provider": route.provider,
                        "status": status.as_u16(),
                        "detail": bounded_text(&error, 512)
                    })),
                )
                    .into_response()
            }
        };
''',
    "bounded upstream response",
)
router = replace_once(
    router,
    '''        if !matches!(parsed_url.scheme(), "http" | "https")
            || parsed_url.host_str().is_none()
            || parsed_url.username() != ""
            || parsed_url.password().is_some()
            || parsed_url.query().is_some()
            || parsed_url.fragment().is_some()
        {
            return Err(format!(
                "executor route {:?} URL must be a credential-free http(s) origin without query or fragment",
                raw.name
            ));
        }
''',
    '''        if !matches!(parsed_url.scheme(), "http" | "https")
            || parsed_url.host_str().is_none()
            || parsed_url.username() != ""
            || parsed_url.password().is_some()
            || parsed_url.query().is_some()
            || parsed_url.fragment().is_some()
            || parsed_url.path() != "/"
        {
            return Err(format!(
                "executor route {:?} URL must be a credential-free http(s) origin without path, query, or fragment",
                raw.name
            ));
        }
        let host = parsed_url.host_str().expect("checked host");
        if parsed_url.scheme() == "http" && !host.ends_with(".svc.cluster.local") {
            return Err(format!(
                "executor route {:?} may use plain HTTP only for an in-cluster .svc.cluster.local origin",
                raw.name
            ));
        }
''',
    "route origin validation",
)
router = replace_once(
    router,
    '''    let request_id = object
        .get("requestId")
''',
    '''    let repo_url = object
        .get("repoUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| "repoUrl must be a credential-free HTTPS origin/path".to_string())?;
    let parsed_repo = reqwest::Url::parse(repo_url)
        .map_err(|_| "repoUrl must be a credential-free HTTPS origin/path".to_string())?;
    if parsed_repo.scheme() != "https"
        || parsed_repo.host_str().is_none()
        || parsed_repo.username() != ""
        || parsed_repo.password().is_some()
        || parsed_repo.query().is_some()
        || parsed_repo.fragment().is_some()
    {
        return Err("repoUrl must be a credential-free HTTPS origin/path".to_string());
    }
    if !matches!(object.get("executor"), None | Some(Value::Null))
        && object.get("executor").and_then(Value::as_str) != Some("local")
    {
        return Err("run-profile routing requires executor=local".to_string());
    }
    if !matches!(object.get("image"), None | Some(Value::Null))
        && object.get("image").and_then(Value::as_str) != Some("")
    {
        return Err("run-profile routing does not accept image".to_string());
    }
    if object
        .get("push")
        .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
    {
        return Err("run-profile routing does not accept push=true".to_string());
    }
    for field in ["deploy", "buildArgs", "dockerfile"] {
        if object.get(field).is_some_and(|value| !value.is_null()) {
            return Err(format!("run-profile routing does not accept {field}"));
        }
    }
    if !matches!(object.get("contextDir"), None | Some(Value::Null))
        && object.get("contextDir").and_then(Value::as_str) != Some(".")
    {
        return Err("run-profile routing accepts only contextDir=.".to_string());
    }
    let request_id = object
        .get("requestId")
''',
    "fixed profile request shape",
)
router = replace_once(
    router,
    '''fn retryable_status(status: StatusCode) -> bool {
''',
    '''async fn read_bounded_response_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Bytes, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!(
            "executor response exceeds {max_bytes} bytes by Content-Length"
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("executor response read failed: {error}"))?
    {
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| "executor response size overflowed".to_string())?;
        if next_len > max_bytes {
            return Err(format!("executor response exceeds {max_bytes} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn retryable_status(status: StatusCode) -> bool {
''',
    "bounded response helper",
)
router = replace_once(
    router,
    '''fn env_usize(name: &str, default: usize) -> Result<usize, String> {
''',
    '''fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(format!("{name} must be true, false, 1, or 0")),
        },
        Err(_) => Ok(default),
    }
}

fn env_usize(name: &str, default: usize) -> Result<usize, String> {
''',
    "boolean environment parser",
)
router = router.replace("upstream-secret-1234", "executor-test-auth-value")
router = router.replace("incoming-secret-1234", "router-test-auth-value")
router = replace_once(
    router,
    '''                routes,
                max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            }),
''',
    '''                routes,
                max_body_bytes: DEFAULT_MAX_BODY_BYTES,
                max_upstream_response_bytes: DEFAULT_MAX_UPSTREAM_RESPONSE_BYTES,
                failover_enabled: true,
                durable_idempotency_certified: true,
            }),
''',
    "test config fields",
)
router = replace_once(
    router,
    '''    fn build_request() -> String {
''',
    '''    fn test_state_without_failover(routes: Vec<ExecutorRoute>) -> AppState {
        let mut state = test_state(routes);
        let mut config = (*state.config).clone();
        config.failover_enabled = false;
        config.durable_idempotency_certified = false;
        state.config = Arc::new(config);
        state
    }

    fn build_request() -> String {
''',
    "single-route test state",
)
router = replace_once(
    router,
    '''        let missing_secret = r#"[
          {"name":"aws","provider":"aws","url":"https://a.example","authEnv":"AUTH_A","priority":1,"profiles":["rust-verify"]}
        ]"#;
        assert!(parse_routes(missing_secret, |_| None)
            .unwrap_err()
            .contains("requires a secret"));
''',
    '''        let missing_secret = r#"[
          {"name":"aws","provider":"aws","url":"https://a.example","authEnv":"AUTH_A","priority":1,"profiles":["rust-verify"]}
        ]"#;
        assert!(parse_routes(missing_secret, |_| None)
            .unwrap_err()
            .contains("requires a secret"));

        let external_http = r#"[
          {"name":"bad-http","provider":"hetzner","url":"http://executor.example","authEnv":"AUTH_A","priority":1,"profiles":["rust-verify"]}
        ]"#;
        assert!(parse_routes(external_http, |_| Some("executor-test-auth-value".into()))
            .unwrap_err()
            .contains("plain HTTP only"));

        let path_bearing = r#"[
          {"name":"bad-path","provider":"aws","url":"https://executor.example/api","authEnv":"AUTH_A","priority":1,"profiles":["rust-verify"]}
        ]"#;
        assert!(parse_routes(path_bearing, |_| Some("executor-test-auth-value".into()))
            .unwrap_err()
            .contains("without path"));
''',
    "route security tests",
)
router = replace_once(
    router,
    '''    #[test]
    fn failover_statuses_are_explicit_and_bounded() {
''',
    '''    #[test]
    fn failover_requires_durable_idempotency_certification() {
        assert!(validate_failover_policy(false, false).is_ok());
        assert!(validate_failover_policy(true, true).is_ok());
        assert!(validate_failover_policy(true, false)
            .unwrap_err()
            .contains("DURABLE_IDEMPOTENCY_CERTIFIED=true"));
    }

    #[test]
    fn failover_statuses_are_explicit_and_bounded() {
''',
    "failover certification test",
)
router = replace_once(
    router,
    '''    #[tokio::test]
    async fn explicit_capacity_failure_fails_over_and_status_proxy_is_stateless() {
''',
    '''    #[tokio::test]
    async fn default_single_authority_mode_never_reaches_secondary_executor() {
        let primary_submit_hits = Arc::new(AtomicUsize::new(0));
        let primary_url = spawn_mock(MockExecutor {
            submit_status: StatusCode::SERVICE_UNAVAILABLE,
            submit_body: json!({"error":"full"}),
            status_body: json!({"id":"unused","status":"failed"}),
            submit_hits: primary_submit_hits.clone(),
            status_hits: Arc::new(AtomicUsize::new(0)),
        })
        .await;
        let secondary_submit_hits = Arc::new(AtomicUsize::new(0));
        let secondary_url = spawn_mock(MockExecutor {
            submit_status: StatusCode::ACCEPTED,
            submit_body: json!({"id":"must-not-run","status":"queued"}),
            status_body: json!({"id":"must-not-run","status":"queued"}),
            submit_hits: secondary_submit_hits.clone(),
            status_hits: Arc::new(AtomicUsize::new(0)),
        })
        .await;
        let app = router(test_state_without_failover(vec![
            route("aws-primary", "aws", &primary_url, 10),
            route("hetzner-secondary", "hetzner", &secondary_url, 20),
        ]));
        let response = call_submit(app).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(primary_submit_hits.load(Ordering::SeqCst), 1);
        assert_eq!(secondary_submit_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn explicit_capacity_failure_fails_over_and_status_proxy_is_stateless() {
''',
    "single-authority integration test",
)
router = replace_once(
    router,
    '''        let mut arbitrary_job = valid;
        arbitrary_job["jobKind"] = json!("build-image");
        assert!(validate_build_request(&arbitrary_job)
            .unwrap_err()
            .contains("run-profile"));
''',
    '''        let mut lambda_executor = valid.clone();
        lambda_executor["executor"] = json!("lambda");
        assert!(validate_build_request(&lambda_executor)
            .unwrap_err()
            .contains("executor=local"));

        let mut arbitrary_job = valid;
        arbitrary_job["jobKind"] = json!("build-image");
        assert!(validate_build_request(&arbitrary_job)
            .unwrap_err()
            .contains("run-profile"));
''',
    "fixed profile executor test",
)
ROUTER.write_text(router)

deployment = DEPLOYMENT.read_text()
deployment = replace_once(
    deployment,
    '''            - name: GHA_EXECUTOR_ROUTER_MAX_BODY_BYTES
              value: "262144"
''',
    '''            - name: GHA_EXECUTOR_ROUTER_MAX_BODY_BYTES
              value: "262144"
            - name: GHA_EXECUTOR_ROUTER_MAX_UPSTREAM_RESPONSE_BYTES
              value: "1048576"
            # Multi-provider failover remains disabled until a durable,
            # cross-provider request-id authority has been certified.
            - name: GHA_EXECUTOR_ROUTER_FAILOVER_ENABLED
              value: "false"
            - name: GHA_EXECUTOR_ROUTER_DURABLE_IDEMPOTENCY_CERTIFIED
              value: "false"
''',
    "deployment failover gate",
)
DEPLOYMENT.write_text(deployment)

contract = CONTRACT.read_text()
contract = replace_once(
    contract,
    '''  assert.match(deployment, /name:\\s*GHA_EXECUTOR_HETZNER_AUTH/);

  assert.match(router, /jobKind=run-profile/);
''',
    '''  assert.match(deployment, /name:\\s*GHA_EXECUTOR_HETZNER_AUTH/);
  assert.match(
    deployment,
    /name:\\s*GHA_EXECUTOR_ROUTER_FAILOVER_ENABLED\\s+value:\\s*"false"/,
  );
  assert.match(
    deployment,
    /name:\\s*GHA_EXECUTOR_ROUTER_DURABLE_IDEMPOTENCY_CERTIFIED\\s+value:\\s*"false"/,
  );

  assert.match(router, /jobKind=run-profile/);
''',
    "deployment contract gate",
)
contract = replace_once(
    contract,
    '''  assert.match(router, /request was not retried/);
  assert.doesNotMatch(router, /Authorization:\\s*Bearer|ghp_[A-Za-z0-9]+/);
''',
    '''  assert.match(router, /request was not retried/);
  assert.match(router, /DURABLE_IDEMPOTENCY_CERTIFIED=true/);
  assert.match(router, /plain HTTP only for an in-cluster/);
  assert.match(router, /executor response exceeds/);
  assert.doesNotMatch(router, /Authorization:\\s*Bearer|ghp_[A-Za-z0-9]+/);
''',
    "router hardening contract",
)
CONTRACT.write_text(contract)

doc = DOC.read_text()
marker = "## Durable failover activation gate"
if marker not in doc:
    doc += '''

## Durable failover activation gate

The router starts in **single-authority mode** even when both AWS and Hetzner
routes are configured. `GHA_EXECUTOR_ROUTER_FAILOVER_ENABLED` defaults to
`false`, so only the highest-priority compatible executor is contacted.

Cross-provider failover is rejected at startup unless both of these reviewed
settings are true:

- `GHA_EXECUTOR_ROUTER_FAILOVER_ENABLED=true`
- `GHA_EXECUTOR_ROUTER_DURABLE_IDEMPOTENCY_CERTIFIED=true`

The certification flag is an operator assertion, not an implementation of
idempotency. It may be enabled only after request identities are reconciled in
a restart-durable shared store and Fiducia-fenced claims prevent AWS and
Hetzner from both becoming authoritative for one logical job. In-process
request-ID deduplication alone is insufficient.

Executor responses are read through a bounded streaming reader. External
routes require HTTPS; plain HTTP is accepted only for Kubernetes service DNS
names ending in `.svc.cluster.local`. Route URLs may not contain credentials,
paths, queries, or fragments.
'''
DOC.write_text(doc)
