#!/usr/bin/env python3
from __future__ import annotations

import re
from pathlib import Path
from textwrap import dedent

ROOT = Path(__file__).resolve().parents[2]
LIB = ROOT / "remote/deployments/gha-executor-router-rs/src/lib.rs"


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one exact match, found {count}")
    return text.replace(old, new, 1)


def replace_between(text: str, start: str, end: str, replacement: str, label: str) -> str:
    start_index = text.find(start)
    if start_index < 0:
        raise SystemExit(f"{label}: start marker missing")
    end_index = text.find(end, start_index + len(start))
    if end_index < 0:
        raise SystemExit(f"{label}: end marker missing")
    return text[:start_index] + replacement + text[end_index:]


def replace_test(text: str, name: str, next_name: str, replacement: str) -> str:
    pattern = re.compile(
        rf"    #\[tokio::test\]\n    async fn {re.escape(name)}\(\) \{{.*?"
        rf"(?=    #\[tokio::test\]\n    async fn {re.escape(next_name)}\(\) \{{)",
        re.DOTALL,
    )
    updated, count = pattern.subn(replacement.rstrip() + "\n\n", text, count=1)
    if count != 1:
        raise SystemExit(f"test {name}: expected one match, found {count}")
    return updated


lib = LIB.read_text(encoding="utf-8")

lib = replace_between(
    lib,
    "#[derive(Clone, Debug, Deserialize)]\n#[serde(rename_all = \"camelCase\", deny_unknown_fields)]\npub struct ExecutorSpec {",
    "\n#[derive(Clone, Debug)]\nstruct Executor {",
    dedent(r'''
    #[derive(Clone, Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct ExecutorSpec {
        pub id: String,
        pub provider: Provider,
        #[serde(default = "default_enabled")]
        pub enabled: bool,
        #[serde(default)]
        pub base_url: Option<String>,
        #[serde(default)]
        pub auth_secret_file: Option<PathBuf>,
    }

    fn default_enabled() -> bool {
        true
    }
    ''').lstrip().rstrip(),
    "ExecutorSpec",
)

lib = replace_between(
    lib,
    "fn build_executors(\n",
    "\nfn validate_executor_set(executors: &[Executor]) -> Result<(), String> {",
    dedent(r'''
    fn build_executors(
        specs: Vec<ExecutorSpec>,
        max_executors: usize,
        execution_enabled: bool,
    ) -> Result<Vec<Executor>, String> {
        if specs.len() > max_executors {
            return Err(format!(
                "configured {} executor identities, above the bounded maximum {max_executors}",
                specs.len()
            ));
        }

        let mut ids = BTreeSet::new();
        let mut providers = BTreeSet::new();
        let mut executors = Vec::with_capacity(specs.len());
        for spec in specs {
            require_executor_id(&spec.id)?;
            if !ids.insert(spec.id.clone()) {
                return Err(format!("duplicate executor id {:?}", spec.id));
            }
            if !providers.insert(spec.provider.as_str()) {
                return Err(format!(
                    "duplicate provider {:?}; configure at most one identity per provider",
                    spec.provider.as_str()
                ));
            }

            if !spec.enabled {
                if spec.base_url.is_some() || spec.auth_secret_file.is_some() {
                    return Err(format!(
                        "disabled executor {} must omit baseUrl and authSecretFile",
                        spec.id
                    ));
                }
                continue;
            }

            let base_url = spec
                .base_url
                .ok_or_else(|| format!("enabled executor {} requires baseUrl", spec.id))?;
            let auth_secret_file = spec
                .auth_secret_file
                .ok_or_else(|| format!("enabled executor {} requires authSecretFile", spec.id))?;
            let base_url = validate_base_url(&base_url)?;
            require_absolute_secret_path(&auth_secret_file, "executor authSecretFile")?;
            let auth_secret = if execution_enabled {
                Some(read_secret_file(
                    &auth_secret_file,
                    &format!("{} executor auth", spec.id),
                )?)
            } else {
                None
            };
            executors.push(Executor {
                id: spec.id,
                provider: spec.provider,
                base_url,
                auth_secret_file,
                auth_secret,
            });
        }
        validate_executor_set(&executors)?;
        Ok(executors)
    }
    ''').lstrip().rstrip(),
    "build_executors",
)

lib = replace_once(
    lib,
    '            .timeout(config.request_timeout)\n            .user_agent("gha-executor-router/0.1")',
    '            .timeout(config.request_timeout)\n'
    '            .redirect(reqwest::redirect::Policy::none())\n'
    '            .user_agent("gha-executor-router/0.1")',
    "redirect policy",
)

lib = lib.replace("fallback_attempts", "readiness_skips")
lib = lib.replace(
    "Retryable pre-acceptance failures that advanced to another executor.",
    "Readiness probes that skipped an executor before any build submission.",
)

new_submit = dedent(r'''
        async fn select_ready_executor(&self) -> Result<Executor, RouterError> {
            for executor in &self.config.executors {
                let auth = executor.auth_secret.as_deref().ok_or_else(|| {
                    RouterError::unavailable("executor_not_ready", "executor auth is unavailable")
                })?;
                let response = self
                    .client
                    .get(format!("{}/readyz", executor.base_url))
                    .header("x-build-server-auth", auth)
                    .send()
                    .await;
                let response = match response {
                    Ok(response) => response,
                    Err(error) => {
                        self.metrics
                            .readiness_skips
                            .fetch_add(1, Ordering::Relaxed);
                        warn!(
                            executor_id = %executor.id,
                            provider = executor.provider.as_str(),
                            error = %error,
                            "executor readiness transport failed before any build submission"
                        );
                        continue;
                    }
                };
                let status = response.status();
                if status != StatusCode::OK {
                    self.metrics
                        .readiness_skips
                        .fetch_add(1, Ordering::Relaxed);
                    warn!(
                        executor_id = %executor.id,
                        provider = executor.provider.as_str(),
                        %status,
                        "executor readiness was not OK before any build submission"
                    );
                    continue;
                }
                let body = match read_bounded(response, self.config.max_response_bytes).await {
                    Ok(body) => body,
                    Err(_) => {
                        self.metrics
                            .readiness_skips
                            .fetch_add(1, Ordering::Relaxed);
                        warn!(
                            executor_id = %executor.id,
                            provider = executor.provider.as_str(),
                            "executor readiness response was unreadable before any build submission"
                        );
                        continue;
                    }
                };
                let ready = serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|value| value.get("ok").and_then(Value::as_bool))
                    .unwrap_or(false);
                if !ready {
                    self.metrics
                        .readiness_skips
                        .fetch_add(1, Ordering::Relaxed);
                    warn!(
                        executor_id = %executor.id,
                        provider = executor.provider.as_str(),
                        "executor readiness body did not assert ok=true"
                    );
                    continue;
                }
                return Ok(executor.clone());
            }

            self.metrics.exhausted.fetch_add(1, Ordering::Relaxed);
            Err(RouterError::unavailable(
                "executors_unavailable",
                "no executor reported ready before any build submission",
            ))
        }

        async fn submit_fresh(&self, request: &BuildRequest) -> Result<Route, RouterError> {
            let executor = self.select_ready_executor().await?;
            let auth = executor.auth_secret.as_deref().ok_or_else(|| {
                RouterError::unavailable("executor_not_ready", "executor auth is unavailable")
            })?;
            let response = self
                .client
                .post(format!("{}/builds", executor.base_url))
                .header("x-build-server-auth", auth)
                .json(request)
                .send()
                .await
                .map_err(|error| {
                    warn!(
                        executor_id = %executor.id,
                        provider = executor.provider.as_str(),
                        error = %error,
                        "executor submission outcome is ambiguous after the POST attempt"
                    );
                    RouterError::bad_gateway(
                        "submission_outcome_ambiguous",
                        format!(
                            "submission to executor {} failed after the POST attempt; fallback was not attempted because work may already exist",
                            executor.id
                        ),
                    )
                })?;

            let status = response.status();
            if status != StatusCode::ACCEPTED {
                if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
                    self.metrics
                        .contract_rejections
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(RouterError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "upstream_contract_rejected",
                        format!(
                            "executor {} rejected the fixed-profile request with HTTP {status}; fallback was not attempted",
                            executor.id
                        ),
                    ));
                }
                return Err(RouterError::bad_gateway(
                    "submission_outcome_ambiguous",
                    format!(
                        "executor {} returned HTTP {status} after the POST attempt; fallback was not attempted because work may already exist",
                        executor.id
                    ),
                ));
            }

            let body = read_bounded(response, self.config.max_response_bytes)
                .await
                .map_err(|_| {
                    RouterError::bad_gateway(
                        "accepted_response_invalid",
                        format!(
                            "executor {} accepted the request but returned an unreadable response; fallback was not attempted",
                            executor.id
                        ),
                    )
                })?;
            let accepted: BuildJob = serde_json::from_slice(&body).map_err(|_| {
                RouterError::bad_gateway(
                    "accepted_response_invalid",
                    format!(
                        "executor {} accepted the request but returned invalid job JSON; fallback was not attempted",
                        executor.id
                    ),
                )
            })?;
            if !safe_token(&accepted.id, 128, b"-_:") {
                return Err(RouterError::bad_gateway(
                    "accepted_response_invalid",
                    format!(
                        "executor {} accepted the request but returned an invalid build id; fallback was not attempted",
                        executor.id
                    ),
                ));
            }
            if !matches!(
                accepted.status.as_str(),
                "queued" | "running" | "succeeded" | "failed"
            ) {
                return Err(RouterError::bad_gateway(
                    "accepted_response_invalid",
                    format!(
                        "executor {} accepted the request but returned an unknown status; fallback was not attempted",
                        executor.id
                    ),
                ));
            }

            self.metrics.accepted.fetch_add(1, Ordering::Relaxed);
            let external_id = format!("{}~{}", executor.id, accepted.id);
            Ok(Route {
                request_id: request.request_id.clone(),
                external_id,
                executor_id: executor.id,
                provider: executor.provider,
                upstream_id: accepted.id.clone(),
                accepted,
                sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            })
        }
''').rstrip()

lib = replace_between(
    lib,
    "    async fn submit_fresh(&self, request: &BuildRequest) -> Result<Route, RouterError> {",
    "\n    async fn insert_route(&self, route: Route) {",
    new_submit,
    "submit_fresh",
)

new_capabilities = dedent(r'''
async fn capabilities(State(engine): State<Engine>) -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "schemaVersion": "build-server.v1",
        "jobKinds": ["run-profile"],
        "providers": engine.config.executors.iter().map(|executor| executor.provider).collect::<Vec<_>>(),
        "failover": {
            "allowed": "only while probing readiness before any POST /builds attempt",
            "readinessSkips": ["transport", "non-200", "unreadable body", "ok is not true"],
            "postSubmissionFailover": false,
            "postAttempt": "transport, timeout, redirect, 429, 5xx, unexpected success, and malformed acceptance all fail closed without contacting another provider",
            "afterAcceptance": "status and artifact access stay pinned; never resubmit"
        },
        "callerSelectedEndpoint": false,
        "callerSelectedCommand": false,
        "callerSelectedImage": false,
        "secretsInline": false,
    }))
}
''').rstrip()

lib = replace_between(
    lib,
    "async fn capabilities(State(engine): State<Engine>) -> Json<Value> {",
    "\nasync fn metrics(State(engine): State<Engine>) -> Response {",
    new_capabilities,
    "capabilities",
)

old_double = r'''    #[derive(Clone)]
    struct DoubleState {
        submit_status: StatusCode,
        poll_status: StatusCode,
        job_id: String,
        submit_body: Value,
        submit_delay: Duration,
        submit_count: Arc<AtomicU64>,
        poll_count: Arc<AtomicU64>,
    }

    impl DoubleState {
        fn new(submit_status: StatusCode, poll_status: StatusCode, job_id: &str) -> Self {
            Self {
                submit_status,
                poll_status,
                job_id: job_id.to_string(),
                submit_body: json!({ "error": "upstream-secret-body-must-not-leak" }),
                submit_delay: Duration::ZERO,
                submit_count: Arc::new(AtomicU64::new(0)),
                poll_count: Arc::new(AtomicU64::new(0)),
            }
        }
    }
'''
new_double = r'''    #[derive(Clone)]
    struct DoubleState {
        ready_status: StatusCode,
        ready_body: Value,
        submit_status: StatusCode,
        poll_status: StatusCode,
        job_id: String,
        submit_body: Value,
        submit_delay: Duration,
        ready_count: Arc<AtomicU64>,
        submit_count: Arc<AtomicU64>,
        poll_count: Arc<AtomicU64>,
    }

    impl DoubleState {
        fn new(submit_status: StatusCode, poll_status: StatusCode, job_id: &str) -> Self {
            Self {
                ready_status: StatusCode::OK,
                ready_body: json!({ "ok": true }),
                submit_status,
                poll_status,
                job_id: job_id.to_string(),
                submit_body: json!({ "error": "upstream-secret-body-must-not-leak" }),
                submit_delay: Duration::ZERO,
                ready_count: Arc::new(AtomicU64::new(0)),
                submit_count: Arc::new(AtomicU64::new(0)),
                poll_count: Arc::new(AtomicU64::new(0)),
            }
        }
    }
'''
lib = replace_once(lib, old_double, new_double, "DoubleState")

lib = replace_once(
    lib,
    "    async fn double_submit(\n",
    dedent(r'''
        async fn double_ready(State(state): State<DoubleState>) -> Response {
            state.ready_count.fetch_add(1, Ordering::Relaxed);
            (state.ready_status, Json(state.ready_body)).into_response()
        }

        async fn double_submit(
    '''),
    "double_ready",
)

lib = replace_once(
    lib,
    '        let app = Router::new()\n'
    '            .route("/builds", post(double_submit))',
    '        let app = Router::new()\n'
    '            .route("/readyz", get(double_ready))\n'
    '            .route("/builds", post(double_submit))',
    "mock readiness route",
)

test_500 = dedent(r'''
    #[tokio::test]
    async fn aws_500_after_post_is_ambiguous_and_never_falls_through() {
        let (aws_url, aws) = spawn_double(DoubleState::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::OK,
            "aws-job",
        ))
        .await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let engine = Engine::new(config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]))
        .unwrap();

        let error = engine.submit(request("request-two")).await.unwrap_err();
        assert_eq!(error.code, "submission_outcome_ambiguous");
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);
    }
''')
lib = replace_test(lib, "aws_500_falls_through_to_hetzner", "aws_429_falls_through_to_hetzner", test_500)

test_429 = dedent(r'''
    #[tokio::test]
    async fn aws_429_after_post_is_ambiguous_and_never_falls_through() {
        let (aws_url, aws) = spawn_double(DoubleState::new(
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::OK,
            "aws-job",
        ))
        .await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let engine = Engine::new(config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]))
        .unwrap();

        let error = engine.submit(request("request-three")).await.unwrap_err();
        assert_eq!(error.code, "submission_outcome_ambiguous");
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);
    }
''')
lib = replace_test(lib, "aws_429_falls_through_to_hetzner", "transport_failure_falls_through_before_acceptance", test_429)

test_transport = dedent(r'''
    #[tokio::test]
    async fn readiness_failure_selects_hetzner_before_any_aws_submission() {
        let mut aws_state = DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "aws-job",
        );
        aws_state.ready_status = StatusCode::SERVICE_UNAVAILABLE;
        aws_state.ready_body = json!({ "ok": false });
        let (aws_url, aws) = spawn_double(aws_state).await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let engine = Engine::new(config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]))
        .unwrap();

        let accepted = engine.submit(request("request-four")).await.unwrap();
        assert_eq!(accepted.id, "hetzner~hetzner-job");
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn post_timeout_after_readiness_is_ambiguous_and_never_falls_through() {
        let mut aws_state = DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "aws-job",
        );
        aws_state.submit_delay = Duration::from_millis(250);
        let (aws_url, aws) = spawn_double(aws_state).await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let mut router_config = config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]);
        router_config.request_timeout = Duration::from_millis(50);
        let engine = Engine::new(router_config).unwrap();

        let error = engine.submit(request("request-timeout")).await.unwrap_err();
        assert_eq!(error.code, "submission_outcome_ambiguous");
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);
    }
''')
lib = replace_test(
    lib,
    "transport_failure_falls_through_before_acceptance",
    "aws_4xx_fails_closed_without_hetzner_fallback_or_body_leak",
    test_transport,
)

lib = replace_once(
    lib,
    '        assert_eq!(accepted.id, "aws~aws-job");\n'
    '        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);\n'
    '        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);',
    '        assert_eq!(accepted.id, "aws~aws-job");\n'
    '        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);\n'
    '        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);\n'
    '        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);\n'
    '        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);',
    "AWS acceptance readiness assertions",
)

lib = replace_once(
    lib,
    '        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);\n'
    '        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);\n'
    '    }\n\n    #[tokio::test]\n    async fn duplicate_request_id_is_submitted_once_even_when_concurrent()',
    '        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);\n'
    '        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);\n'
    '        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);\n'
    '        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);\n'
    '    }\n\n    #[tokio::test]\n'
    '    async fn duplicate_request_id_is_submitted_once_even_when_concurrent()',
    "4xx readiness assertions",
)

disabled_test = dedent(r'''
    #[test]
    fn disabled_executor_identity_must_omit_endpoint_and_secret_state() {
        let disabled = ExecutorSpec {
            id: "hetzner".to_string(),
            provider: Provider::Hetzner,
            enabled: false,
            base_url: None,
            auth_secret_file: None,
        };
        assert!(build_executors(vec![disabled], 2, false).unwrap().is_empty());

        let invalid = ExecutorSpec {
            id: "hetzner".to_string(),
            provider: Provider::Hetzner,
            enabled: false,
            base_url: Some("https://dormant.example.com".to_string()),
            auth_secret_file: None,
        };
        assert!(build_executors(vec![invalid], 2, false)
            .unwrap_err()
            .contains("must omit baseUrl and authSecretFile"));
    }

''')
marker = "    #[test]\n    fn fixed_profile_request_rejects_mutable_or_arbitrary_inputs() {"
if marker not in lib:
    raise SystemExit("fixed-profile test marker missing")
lib = lib.replace(marker, disabled_test + marker, 1)

for required in [
    "select_ready_executor",
    "submission_outcome_ambiguous",
    "postSubmissionFailover",
    "readiness_failure_selects_hetzner_before_any_aws_submission",
    "aws_500_after_post_is_ambiguous_and_never_falls_through",
    "post_timeout_after_readiness_is_ambiguous_and_never_falls_through",
    "disabled executor {} must omit baseUrl and authSecretFile",
    "Policy::none()",
]:
    if required not in lib:
        raise SystemExit(f"patched lib missing {required}")
for forbidden in [
    "aws_500_falls_through_to_hetzner",
    "aws_429_falls_through_to_hetzner",
    "transport_failure_falls_through_before_acceptance",
    '"retryable": ["transport", "429", "5xx"]',
]:
    if forbidden in lib:
        raise SystemExit(f"unsafe legacy contract remains: {forbidden}")

LIB.write_text(lib, encoding="utf-8")
