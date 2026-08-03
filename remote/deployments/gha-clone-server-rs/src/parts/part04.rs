
fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn reject(state: &AppState, status: StatusCode, message: &'static str) -> Response {
    state.counters.rejected.fetch_add(1, Ordering::Relaxed);
    (status, Json(json!({ "error": message }))).into_response()
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/webhooks/github", post(github_webhook))
        .route("/ci/github/webhook", post(github_webhook))
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BYTES))
        .with_state(state)
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

#[tokio::main]
async fn main() {
    init_tracing();
    let config = match Config::from_env().await {
        Ok(value) => value,
        Err(error_message) => {
            eprintln!("{SERVICE_NAME}: configuration error: {error_message}");
            std::process::exit(2);
        }
    };
    let bind = config.bind;
    let cache_size = config.delivery_cache_size;
    let state = AppState {
        config: Arc::new(config),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("reqwest client"),
        deliveries: Arc::new(Mutex::new(DeliveryCache::new(cache_size))),
        counters: Arc::new(Counters::default()),
    };
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {bind}: {error}"));
    info!(%bind, rules = state.config.rules.len(), dry_run = state.config.dry_run, "listening");
    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server exited cleanly");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    fn rules() -> Vec<Rule> {
        parse_rules(
            r#"[
              {
                "repo": "ORESoftware/k8s-cluster",
                "workflow": "repo checks",
                "branches": ["main", "dev"],
                "sourceEvents": ["push"],
                "conclusions": ["failure", "timed_out"],
                "action": {
                  "kind": "workflowDispatch",
                  "workflowFile": "self-hosted-fallback.yml",
                  "workflowName": "Self-hosted fallback",
                  "dispatchRef": "main",
                  "runner": "oresoftware-ci"
                }
              }
            ]"#,
        )
        .expect("valid rules")
    }

    fn state() -> AppState {
        AppState {
            config: Arc::new(Config {
                bind: DEFAULT_BIND.parse().unwrap(),
                webhook_secret: SECRET.to_string(),
                github_token: None,
                build_server_url: DEFAULT_BUILD_SERVER_URL.to_string(),
                build_server_auth: None,
                dry_run: true,
                rules: rules(),
                delivery_cache_size: 100,
            }),
            http: reqwest::Client::new(),
            deliveries: Arc::new(Mutex::new(DeliveryCache::new(100))),
            counters: Arc::new(Counters::default()),
        }
    }

    fn signature(body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn event(conclusion: &str, source_event: &str, workflow: &str) -> Value {
        json!({
            "action": "completed",
            "repository": { "full_name": "ORESoftware/k8s-cluster" },
            "workflow_run": {
                "id": 42,
                "name": workflow,
                "event": source_event,
                "head_branch": "main",
                "head_sha": "0123456789abcdef0123456789abcdef01234567",
                "conclusion": conclusion,
                "run_attempt": 1,
                "head_repository": { "full_name": "ORESoftware/k8s-cluster" }
            }
        })
    }

    async fn send(state: AppState, delivery: &str, body: Value, valid_sig: bool) -> (StatusCode, String) {
        let body = body.to_string();
        let sig = if valid_sig {
            signature(body.as_bytes())
        } else {
            "sha256=deadbeef".to_string()
        };
        let request = Request::builder()
            .method("POST")
            .uri("/webhooks/github")
            .header("content-type", "application/json")
            .header("x-github-event", "workflow_run")
            .header("x-github-delivery", delivery)
            .header("x-hub-signature-256", sig)
            .body(Body::from(body))
            .unwrap();
        let response = build_router(state).oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, String::from_utf8_lossy(&body).to_string())
    }

    #[test]
    fn rules_reject_success_and_recursive_fallbacks() {
        let success = r#"[{
          "repo":"ORESoftware/k8s-cluster","workflow":"repo checks",
          "conclusions":["success"],
          "action":{"kind":"workflowDispatch","workflowFile":"self-hosted-fallback.yml",
          "workflowName":"Self-hosted fallback","dispatchRef":"main","runner":"oresoftware-ci"}
        }]"#;
        assert!(parse_rules(success).is_err());

        let recursive = r#"[{
          "repo":"ORESoftware/k8s-cluster","workflow":"Self-hosted fallback",
          "action":{"kind":"workflowDispatch","workflowFile":"self-hosted-fallback.yml",
          "workflowName":"Self-hosted fallback","dispatchRef":"main","runner":"oresoftware-ci"}
        }]"#;
        assert!(parse_rules(recursive).is_err());
    }

    #[test]
    fn signature_and_sha_validation_fail_closed() {
        let body = br#"{"hello":"world"}"#;
        assert!(verify_github_signature(SECRET, body, &signature(body)));
        assert!(!verify_github_signature(SECRET, body, "sha256=00"));
        assert!(valid_commit_sha("0123456789abcdef0123456789abcdef01234567"));
        assert!(!valid_commit_sha("../../main"));
        assert!(!valid_commit_sha("ABCDEF0123456789abcdef0123456789abcdef01"));
    }

    #[tokio::test]
    async fn bad_signature_is_rejected() {
        let (status, _) = send(state(), "delivery-1", event("failure", "push", "repo checks"), false).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn matching_failure_is_accepted_in_dry_run() {
        let (status, body) = send(state(), "delivery-2", event("failure", "push", "repo checks"), true).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(body.contains("workflow-dispatch"));
        assert!(body.contains("\"dryRun\":true"));
    }

    #[tokio::test]
    async fn success_and_pull_request_are_ignored() {
        for (delivery, payload) in [
            ("delivery-success", event("success", "push", "repo checks")),
            ("delivery-pr", event("failure", "pull_request", "repo checks")),
        ] {
            let (status, body) = send(state(), delivery, payload, true).await;
            assert_eq!(status, StatusCode::OK);
            assert!(body.contains("ignored"));
        }
    }

    #[tokio::test]
    async fn missing_head_repository_is_ignored() {
        let mut payload = event("failure", "push", "repo checks");
        payload["workflow_run"]
            .as_object_mut()
            .expect("workflow_run object")
            .remove("head_repository");
        let (status, body) = send(state(), "delivery-no-head-repo", payload, true).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("ignored-head-repository"));
    }

    #[tokio::test]
    async fn fork_origin_and_duplicate_delivery_are_ignored() {
        let mut fork = event("failure", "push", "repo checks");
        fork["workflow_run"]["head_repository"]["full_name"] =
            Value::String("attacker/k8s-cluster".to_string());
        let (status, body) = send(state(), "delivery-fork", fork, true).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("ignored-fork"));

        let shared = state();
        let payload = event("failure", "push", "repo checks");
        let (first_status, _) = send(shared.clone(), "delivery-duplicate", payload.clone(), true).await;
        let (second_status, second_body) = send(shared, "delivery-duplicate", payload, true).await;
        assert_eq!(first_status, StatusCode::ACCEPTED);
        assert_eq!(second_status, StatusCode::OK);
        assert!(second_body.contains("duplicate"));
    }
}
